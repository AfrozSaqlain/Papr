//! End-to-end encrypted project collaboration over a single GitHub repository.
//!
//! Each project has its own random 32-byte symmetric `ProjectKey` encrypted via
//! `ChaCha20-Poly1305`. The `ProjectKey` is wrapped individually for each
//! authorized collaborator's `ssh-ed25519` public key (converted to X25519 via
//! the birational map and paired with an ephemeral X25519 key via HKDF-SHA256).
//!
//! Removing a collaborator rotates the `ProjectKey` (incrementing
//! `key_generation`) and re-wraps the new key only for the remaining
//! collaborators, preventing removed collaborators from decrypting subsequent
//! project bundles.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use base64::{
    Engine,
    engine::general_purpose::{STANDARD as BASE64_STANDARD, STANDARD_NO_PAD as BASE64_NO_PAD},
};
use chrono::Utc;
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use ring::{
    aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey},
    hkdf::{HKDF_SHA256, KeyType, Salt},
    rand::{SecureRandom, SystemRandom},
    signature::{Ed25519KeyPair, KeyPair},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use thiserror::Error;
use walkdir::WalkDir;

/// Hidden metadata file inside a project folder storing its [`ProjectKeyEnvelope`].
pub const LOCAL_COLLAB_FILE: &str = ".papr-collab.json";

/// Hidden local cache of the unwrapped 32-byte project key for offline edits.
pub const LOCAL_PROJECT_KEY_FILE: &str = ".papr-project-key";

/// Default filename for Papr's managed OpenSSH Ed25519 private key.
pub const PAPR_SSH_PRIVATE_KEY_FILE: &str = "id_ed25519";

/// Default filename for Papr's managed OpenSSH Ed25519 public key.
pub const PAPR_SSH_PUBLIC_KEY_FILE: &str = "id_ed25519.pub";

const OPENSSH_AUTH_MAGIC: &[u8] = b"openssh-key-v1\0";
const SSH_ED25519_KEY_TYPE: &str = "ssh-ed25519";
const KEY_WRAP_INFO: &[u8] = b"papr-project-key-v1";
const BUNDLE_MAGIC: &[u8] = b"PAPRENC1";

/// Errors produced during project encryption, key exchange, or bundle unpacking.
#[derive(Debug, Error)]
pub enum CollabError {
    /// Standard I/O error.
    #[error("collaboration I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization or deserialization failure.
    #[error("collaboration JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Base64 decoding error.
    #[error("collaboration base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),
    /// Invalid or unsupported SSH key format.
    #[error("invalid SSH Ed25519 key: {0}")]
    InvalidSshKey(String),
    /// Cryptographic primitive failure (encryption, decryption, or CSPRNG).
    #[error("cryptographic operation failed: {0}")]
    Crypto(String),
    /// Current user's private keys are not authorized in the project's key envelope.
    #[error("unauthorized: none of your local SSH keys have access to project '{0}'")]
    Unauthorized(String),
    /// Invalid collaborator username.
    #[error("invalid GitHub username: '{0}'")]
    InvalidUsername(String),
    /// Unsafe or malformed relative file path inside a bundle.
    #[error("invalid relative path in project bundle: '{0}'")]
    InvalidBundlePath(String),
}

/// An OpenSSH `ssh-ed25519` public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshEd25519PublicKey {
    /// Raw 32-byte Ed25519 public key point ($y$ coordinate with sign bit).
    ed25519_bytes: [u8; 32],
    /// Optional trailing comment (e.g. `user@host`).
    comment: Option<String>,
}

impl SshEd25519PublicKey {
    /// Construct from raw 32-byte Ed25519 public key bytes.
    #[must_use]
    pub fn from_ed25519_bytes(ed25519_bytes: [u8; 32], comment: Option<String>) -> Self {
        Self {
            ed25519_bytes,
            comment,
        }
    }

    /// Parse a single OpenSSH `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5...` line.
    ///
    /// # Errors
    /// Returns [`CollabError::InvalidSshKey`] if the line is not a valid `ssh-ed25519` public key.
    pub fn parse_openssh_line(line: &str) -> Result<Self, CollabError> {
        let trimmed = line.trim();
        let mut parts = trimmed.split_whitespace();
        let key_type = parts
            .next()
            .ok_or_else(|| CollabError::InvalidSshKey("empty key line".into()))?;
        if key_type != SSH_ED25519_KEY_TYPE {
            return Err(CollabError::InvalidSshKey(format!(
                "expected '{SSH_ED25519_KEY_TYPE}', found '{key_type}'"
            )));
        }
        let b64_blob = parts
            .next()
            .ok_or_else(|| CollabError::InvalidSshKey("missing base64 key payload".into()))?;
        let comment = parts.collect::<Vec<_>>().join(" ");
        let comment = (!comment.is_empty()).then_some(comment);

        let blob = BASE64_STANDARD.decode(b64_blob)?;
        let mut cursor = 0;
        let wire_type = read_ssh_string(&blob, &mut cursor)?;
        if wire_type != SSH_ED25519_KEY_TYPE.as_bytes() {
            return Err(CollabError::InvalidSshKey(
                "mismatched wire key type".into(),
            ));
        }
        let raw_key = read_ssh_string(&blob, &mut cursor)?;
        let ed25519_bytes: [u8; 32] = raw_key
            .try_into()
            .map_err(|_| CollabError::InvalidSshKey("Ed25519 public key must be 32 bytes".into()))?;

        // Validate that conversion to X25519 succeeds.
        let _ = ed25519_pubkey_to_x25519(&ed25519_bytes)?;

        Ok(Self {
            ed25519_bytes,
            comment,
        })
    }

    /// Extract all valid `ssh-ed25519` public keys from a multi-line `.keys` response.
    ///
    /// Non-Ed25519 keys (such as `ssh-rsa` or `ecdsa-sha2-nistp256`) and blank lines
    /// are ignored so mixed GitHub `.keys` files work seamlessly.
    #[must_use]
    pub fn parse_many_lines(text: &str) -> Vec<Self> {
        let mut keys = Vec::new();
        for line in text.lines() {
            if let Ok(key) = Self::parse_openssh_line(line)
                && !keys
                    .iter()
                    .any(|existing: &Self| existing.ed25519_bytes == key.ed25519_bytes)
            {
                keys.push(key);
            }
        }
        keys
    }

    /// Return the raw 32-byte Ed25519 public key bytes.
    #[must_use]
    pub const fn ed25519_bytes(&self) -> &[u8; 32] {
        &self.ed25519_bytes
    }

    /// Convert this Ed25519 public key into its birationally equivalent 32-byte X25519 public key.
    ///
    /// # Errors
    /// Returns [`CollabError::InvalidSshKey`] if the Edwards point cannot be mapped.
    pub fn to_x25519_bytes(&self) -> Result<[u8; 32], CollabError> {
        ed25519_pubkey_to_x25519(&self.ed25519_bytes)
    }

    /// Standard OpenSSH wire blob (`string "ssh-ed25519" || string <32-byte key>`).
    #[must_use]
    pub fn to_wire_blob(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(51);
        write_ssh_string(&mut blob, SSH_ED25519_KEY_TYPE.as_bytes());
        write_ssh_string(&mut blob, &self.ed25519_bytes);
        blob
    }

    /// Standard OpenSSH `SHA256:...` fingerprint of the public key wire blob.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let digest = Sha256::digest(self.to_wire_blob());
        format!("SHA256:{}", BASE64_NO_PAD.encode(digest))
    }

    /// Format as a single-line OpenSSH `.pub` string (`ssh-ed25519 AAAA... [comment]`).
    #[must_use]
    pub fn to_openssh_line(&self) -> String {
        let b64 = BASE64_STANDARD.encode(self.to_wire_blob());
        if let Some(comment) = &self.comment
            && !comment.trim().is_empty()
        {
            format!("{SSH_ED25519_KEY_TYPE} {b64} {}", comment.trim())
        } else {
            format!("{SSH_ED25519_KEY_TYPE} {b64}")
        }
    }
}

/// An OpenSSH `ssh-ed25519` private/public keypair.
#[derive(Clone)]
pub struct SshEd25519KeyPair {
    seed: [u8; 32],
    public_key: SshEd25519PublicKey,
}

impl std::fmt::Debug for SshEd25519KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshEd25519KeyPair")
            .field("public_key", &self.public_key.fingerprint())
            .finish_non_exhaustive()
    }
}

impl SshEd25519KeyPair {
    /// Create a keypair from a 32-byte Ed25519 secret seed.
    ///
    /// # Errors
    /// Returns [`CollabError::Crypto`] if `ring` rejects the seed.
    pub fn from_seed(seed: [u8; 32], comment: Option<String>) -> Result<Self, CollabError> {
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed)
            .map_err(|_| CollabError::Crypto("invalid Ed25519 seed".into()))?;
        let pub_slice = pair.public_key().as_ref();
        let ed25519_bytes: [u8; 32] = pub_slice
            .try_into()
            .map_err(|_| CollabError::Crypto("invalid Ed25519 public key length".into()))?;
        Ok(Self {
            seed,
            public_key: SshEd25519PublicKey::from_ed25519_bytes(ed25519_bytes, comment),
        })
    }

    /// Generate a brand new random `ssh-ed25519` keypair.
    ///
    /// # Errors
    /// Returns [`CollabError::Crypto`] if system randomness fails.
    pub fn generate(comment: Option<String>) -> Result<Self, CollabError> {
        let rng = SystemRandom::new();
        let mut seed = [0u8; 32];
        rng.fill(&mut seed)
            .map_err(|_| CollabError::Crypto("failed to generate random Ed25519 seed".into()))?;
        Self::from_seed(seed, comment)
    }

    /// Parse an unencrypted OpenSSH `-----BEGIN OPENSSH PRIVATE KEY-----` file.
    ///
    /// # Errors
    /// Returns [`CollabError::InvalidSshKey`] if the file is not an unencrypted `ssh-ed25519` key.
    pub fn parse_openssh_private_key(pem: &str) -> Result<Self, CollabError> {
        let mut b64_body = String::new();
        let mut inside = false;
        for line in pem.lines() {
            let trimmed = line.trim();
            if trimmed == "-----BEGIN OPENSSH PRIVATE KEY-----" {
                inside = true;
                continue;
            }
            if trimmed == "-----END OPENSSH PRIVATE KEY-----" {
                break;
            }
            if inside {
                b64_body.push_str(trimmed);
            }
        }
        if b64_body.is_empty() {
            return Err(CollabError::InvalidSshKey(
                "missing OPENSSH PRIVATE KEY PEM block".into(),
            ));
        }

        let raw = BASE64_STANDARD.decode(&b64_body)?;
        if !raw.starts_with(OPENSSH_AUTH_MAGIC) {
            return Err(CollabError::InvalidSshKey(
                "invalid openssh-key-v1 magic header".into(),
            ));
        }
        let mut cursor = OPENSSH_AUTH_MAGIC.len();
        let ciphername = read_ssh_string(&raw, &mut cursor)?;
        let kdfname = read_ssh_string(&raw, &mut cursor)?;
        let _kdfoptions = read_ssh_string(&raw, &mut cursor)?;
        if ciphername != b"none" || kdfname != b"none" {
            return Err(CollabError::InvalidSshKey(
                "encrypted SSH private key requires a passphrase; Papr will use its managed key"
                    .into(),
            ));
        }
        let num_keys = read_ssh_u32(&raw, &mut cursor)?;
        if num_keys == 0 {
            return Err(CollabError::InvalidSshKey(
                "OpenSSH private key contains zero keys".into(),
            ));
        }
        // Skip public key blob(s)
        for _ in 0..num_keys {
            let _pub_blob = read_ssh_string(&raw, &mut cursor)?;
        }
        let private_section = read_ssh_string(&raw, &mut cursor)?;
        let mut priv_cursor = 0;
        let check1 = read_ssh_u32(private_section, &mut priv_cursor)?;
        let check2 = read_ssh_u32(private_section, &mut priv_cursor)?;
        if check1 != check2 {
            return Err(CollabError::InvalidSshKey(
                "OpenSSH private key checkint mismatch".into(),
            ));
        }
        let key_type = read_ssh_string(private_section, &mut priv_cursor)?;
        if key_type != SSH_ED25519_KEY_TYPE.as_bytes() {
            return Err(CollabError::InvalidSshKey(
                "OpenSSH private key is not ssh-ed25519".into(),
            ));
        }
        let _pub_bytes = read_ssh_string(private_section, &mut priv_cursor)?;
        let priv_bytes = read_ssh_string(private_section, &mut priv_cursor)?;
        if priv_bytes.len() != 64 {
            return Err(CollabError::InvalidSshKey(
                "Ed25519 private key payload must be 64 bytes".into(),
            ));
        }
        let comment_bytes = read_ssh_string(private_section, &mut priv_cursor)?;
        let comment = String::from_utf8(comment_bytes.to_vec())
            .ok()
            .filter(|s| !s.is_empty());

        let mut seed = [0u8; 32];
        seed.copy_from_slice(&priv_bytes[..32]);
        Self::from_seed(seed, comment)
    }

    /// Serialize this keypair as a standard unencrypted `-----BEGIN OPENSSH PRIVATE KEY-----` PEM string.
    #[must_use]
    pub fn to_openssh_private_key_pem(&self) -> String {
        let mut out = Vec::new();
        out.extend_from_slice(OPENSSH_AUTH_MAGIC);
        write_ssh_string(&mut out, b"none");
        write_ssh_string(&mut out, b"none");
        write_ssh_string(&mut out, b"");
        write_ssh_u32(&mut out, 1);

        let pub_blob = self.public_key.to_wire_blob();
        write_ssh_string(&mut out, &pub_blob);

        let mut priv_section = Vec::new();
        let check: u32 = 0x5041_5052; // "PAPR"
        write_ssh_u32(&mut priv_section, check);
        write_ssh_u32(&mut priv_section, check);
        write_ssh_string(&mut priv_section, SSH_ED25519_KEY_TYPE.as_bytes());
        write_ssh_string(&mut priv_section, self.public_key.ed25519_bytes());

        let mut priv_64 = [0u8; 64];
        priv_64[..32].copy_from_slice(&self.seed);
        priv_64[32..].copy_from_slice(self.public_key.ed25519_bytes());
        write_ssh_string(&mut priv_section, &priv_64);

        let comment = self.public_key.comment.as_deref().unwrap_or("papr");
        write_ssh_string(&mut priv_section, comment.as_bytes());

        // Pad to 8-byte block boundary with 1, 2, 3, ...
        let mut pad_byte = 1u8;
        while !priv_section.len().is_multiple_of(8) {
            priv_section.push(pad_byte);
            pad_byte = pad_byte.wrapping_add(1);
        }
        write_ssh_string(&mut out, &priv_section);

        let b64 = BASE64_STANDARD.encode(&out);
        let mut pem = String::from("-----BEGIN OPENSSH PRIVATE KEY-----\n");
        for chunk in b64.as_bytes().chunks(70) {
            if let Ok(line) = std::str::from_utf8(chunk) {
                pem.push_str(line);
                pem.push('\n');
            }
        }
        pem.push_str("-----END OPENSSH PRIVATE KEY-----\n");
        pem
    }

    /// Return the associated [`SshEd25519PublicKey`].
    #[must_use]
    pub const fn public_key(&self) -> &SshEd25519PublicKey {
        &self.public_key
    }

    /// Derive the clamped 32-byte X25519 secret scalar from the Ed25519 seed (`SHA-512(seed)[0..32]`).
    #[must_use]
    pub fn to_x25519_scalar(&self) -> [u8; 32] {
        let digest = Sha512::digest(self.seed);
        let mut scalar = [0u8; 32];
        scalar.copy_from_slice(&digest[..32]);
        clamp_x25519_scalar(&mut scalar);
        scalar
    }
}

/// Discover the user's local `~/.ssh/id_ed25519` key (if available and unencrypted)
/// and ensure a persistent Papr Ed25519 keypair exists inside `papr_data_dir`.
///
/// # Errors
/// Returns [`CollabError`] if `papr_data_dir` cannot be accessed or key generation fails.
pub fn discover_or_create_local_keys(
    papr_data_dir: &Path,
) -> Result<Vec<SshEd25519KeyPair>, CollabError> {
    let mut keys = Vec::new();

    // 1. Check standard ~/.ssh/id_ed25519 if present
    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    {
        let user_ssh_key = home.join(".ssh").join("id_ed25519");
        if user_ssh_key.is_file()
            && let Ok(pem) = fs::read_to_string(&user_ssh_key)
            && let Ok(pair) = SshEd25519KeyPair::parse_openssh_private_key(&pem)
        {
            keys.push(pair);
        }
    }

    // 2. Ensure Papr's managed identity key exists in papr_data_dir
    fs::create_dir_all(papr_data_dir)?;
    let papr_priv_path = papr_data_dir.join(PAPR_SSH_PRIVATE_KEY_FILE);
    let papr_pub_path = papr_data_dir.join(PAPR_SSH_PUBLIC_KEY_FILE);

    let papr_pair = if papr_priv_path.is_file()
        && let Ok(pem) = fs::read_to_string(&papr_priv_path)
        && let Ok(pair) = SshEd25519KeyPair::parse_openssh_private_key(&pem)
    {
        pair
    } else {
        let generated = SshEd25519KeyPair::generate(Some("papr-collab".into()))?;
        fs::write(&papr_priv_path, generated.to_openssh_private_key_pem())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&papr_priv_path, fs::Permissions::from_mode(0o600));
        }
        fs::write(
            &papr_pub_path,
            format!("{}\n", generated.public_key().to_openssh_line()),
        )?;
        generated
    };

    if !keys.iter().any(|k| {
        k.public_key().ed25519_bytes() == papr_pair.public_key().ed25519_bytes()
    }) {
        keys.push(papr_pair);
    }

    Ok(keys)
}

/// A 32-byte symmetric project encryption key (`ChaCha20-Poly1305`).
#[derive(Clone, PartialEq, Eq)]
pub struct ProjectKey([u8; 32]);

impl std::fmt::Debug for ProjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProjectKey([REDACTED])")
    }
}

impl ProjectKey {
    /// Generate a fresh random 32-byte project key.
    ///
    /// # Errors
    /// Returns [`CollabError::Crypto`] if the system CSPRNG fails.
    pub fn generate() -> Result<Self, CollabError> {
        let rng = SystemRandom::new();
        let mut bytes = [0u8; 32];
        rng.fill(&mut bytes)
            .map_err(|_| CollabError::Crypto("failed to generate random project key".into()))?;
        Ok(Self(bytes))
    }

    /// Construct from raw 32-byte key material.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the raw 32-byte key material.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Encode to standard base64 for local caching.
    #[must_use]
    pub fn to_base64(&self) -> String {
        BASE64_STANDARD.encode(self.0)
    }

    /// Decode from standard base64.
    ///
    /// # Errors
    /// Returns [`CollabError`] if the base64 is invalid or not 32 bytes.
    pub fn from_base64(encoded: &str) -> Result<Self, CollabError> {
        let decoded = BASE64_STANDARD.decode(encoded.trim())?;
        let bytes: [u8; 32] = decoded
            .try_into()
            .map_err(|_| CollabError::Crypto("project key must be 32 bytes".into()))?;
        Ok(Self(bytes))
    }
}

/// A copy of the [`ProjectKey`] encrypted for a single recipient's `ssh-ed25519` public key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WrappedProjectKey {
    /// GitHub username of the recipient (lowercase).
    pub username: String,
    /// OpenSSH `SHA256:...` fingerprint of the recipient's `ssh-ed25519` public key.
    pub fingerprint: String,
    /// Base64-encoded 32-byte ephemeral X25519 public key ($E_{\text{pk}}$).
    pub ephemeral_public_key: String,
    /// Base64-encoded 12-byte ChaCha20-Poly1305 nonce.
    pub nonce: String,
    /// Base64-encoded encrypted 32-byte project key + 16-byte authentication tag.
    pub encrypted_key: String,
}

impl WrappedProjectKey {
    /// Wrap `project_key` for `recipient_pub` belonging to `username`.
    ///
    /// # Errors
    /// Returns [`CollabError`] if key conversion or encryption fails.
    pub fn wrap(
        project_key: &ProjectKey,
        username: &str,
        recipient_pub: &SshEd25519PublicKey,
    ) -> Result<Self, CollabError> {
        let recipient_x25519 = recipient_pub.to_x25519_bytes()?;
        let rng = SystemRandom::new();

        let mut eph_scalar = [0u8; 32];
        rng.fill(&mut eph_scalar)
            .map_err(|_| CollabError::Crypto("failed to generate ephemeral scalar".into()))?;
        clamp_x25519_scalar(&mut eph_scalar);

        let eph_pub = x25519_base(&eph_scalar);
        let shared_secret = x25519(&eph_scalar, &recipient_x25519)?;
        let wrapping_key = derive_wrapping_key(&shared_secret, &eph_pub, &recipient_x25519)?;

        let mut nonce_bytes = [0u8; 12];
        rng.fill(&mut nonce_bytes)
            .map_err(|_| CollabError::Crypto("failed to generate wrap nonce".into()))?;

        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &wrapping_key)
            .map_err(|_| CollabError::Crypto("failed to initialize ChaCha20-Poly1305".into()))?;
        let key = LessSafeKey::new(unbound);

        let mut in_out = project_key.as_bytes().to_vec();
        let aad = normalize_username(username);
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(aad.as_bytes()),
            &mut in_out,
        )
        .map_err(|_| CollabError::Crypto("failed to encrypt project key".into()))?;

        Ok(Self {
            username: aad,
            fingerprint: recipient_pub.fingerprint(),
            ephemeral_public_key: BASE64_STANDARD.encode(eph_pub),
            nonce: BASE64_STANDARD.encode(nonce_bytes),
            encrypted_key: BASE64_STANDARD.encode(in_out),
        })
    }

    /// Attempt to unwrap the [`ProjectKey`] using `local_keypair`.
    ///
    /// # Errors
    /// Returns [`CollabError`] if the keypair does not match or authentication fails.
    pub fn unwrap(&self, local_keypair: &SshEd25519KeyPair) -> Result<ProjectKey, CollabError> {
        let eph_pub_vec = BASE64_STANDARD.decode(&self.ephemeral_public_key)?;
        let eph_pub: [u8; 32] = eph_pub_vec
            .try_into()
            .map_err(|_| CollabError::Crypto("ephemeral public key must be 32 bytes".into()))?;

        let nonce_vec = BASE64_STANDARD.decode(&self.nonce)?;
        let nonce_bytes: [u8; 12] = nonce_vec
            .try_into()
            .map_err(|_| CollabError::Crypto("nonce must be 12 bytes".into()))?;

        let mut ciphertext = BASE64_STANDARD.decode(&self.encrypted_key)?;

        let local_scalar = local_keypair.to_x25519_scalar();
        let local_x25519_pub = local_keypair.public_key().to_x25519_bytes()?;
        let shared_secret = x25519(&local_scalar, &eph_pub)?;
        let wrapping_key = derive_wrapping_key(&shared_secret, &eph_pub, &local_x25519_pub)?;

        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &wrapping_key)
            .map_err(|_| CollabError::Crypto("failed to initialize ChaCha20-Poly1305".into()))?;
        let key = LessSafeKey::new(unbound);

        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(self.username.as_bytes()),
                &mut ciphertext,
            )
            .map_err(|_| CollabError::Crypto("failed to unwrap project key".into()))?;

        let key_bytes: [u8; 32] = (&*plaintext)
            .try_into()
            .map_err(|_| CollabError::Crypto("unwrapped project key is not 32 bytes".into()))?;
        Ok(ProjectKey::from_bytes(key_bytes))
    }
}

/// Key envelope stored at `projects/<project_name>/keys.json` in the shared GitHub repository
/// and cached locally at `.papr-collab.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectKeyEnvelope {
    /// Schema format version.
    pub version: u32,
    /// Canonical project name.
    pub project_name: String,
    /// Project compiler (`"typst"` or `"latex"`).
    pub compiler: String,
    /// GitHub username of the project creator/owner.
    pub owner: String,
    /// Authorized collaborator GitHub usernames (lowercase, excluding `owner`).
    pub collaborators: Vec<String>,
    /// Key rotation generation counter (increments whenever a collaborator is removed).
    pub key_generation: u32,
    /// ISO 8601 timestamp of the last envelope or bundle update.
    pub updated_at: String,
    /// Wrapped copies of the current `ProjectKey` for `owner` and `collaborators`.
    pub recipient_keys: Vec<WrappedProjectKey>,
    /// Cached OpenSSH `ssh-ed25519` public key lines per username so key rotation works offline.
    #[serde(default)]
    pub public_key_cache: BTreeMap<String, Vec<String>>,
}

impl ProjectKeyEnvelope {
    /// Create a new key envelope for `project_name` owned by `owner`.
    ///
    /// # Errors
    /// Returns [`CollabError`] if wrapping the initial project key fails.
    pub fn new(
        project_name: &str,
        compiler: &str,
        owner: &str,
        owner_keys: &[SshEd25519PublicKey],
        project_key: &ProjectKey,
    ) -> Result<Self, CollabError> {
        let owner_norm = normalize_username(owner);
        let mut recipient_keys = Vec::new();
        let mut public_key_cache = BTreeMap::new();

        let mut cached_lines = Vec::new();
        for key in owner_keys {
            recipient_keys.push(WrappedProjectKey::wrap(project_key, &owner_norm, key)?);
            cached_lines.push(key.to_openssh_line());
        }
        if !cached_lines.is_empty() {
            public_key_cache.insert(owner_norm.clone(), cached_lines);
        }

        Ok(Self {
            version: 1,
            project_name: project_name.to_string(),
            compiler: compiler.to_string(),
            owner: owner_norm,
            collaborators: Vec::new(),
            key_generation: 1,
            updated_at: Utc::now().to_rfc3339(),
            recipient_keys,
            public_key_cache,
        })
    }

    /// Check whether `username` is the owner or an authorized collaborator.
    #[must_use]
    pub fn is_authorized_user(&self, username: &str) -> bool {
        let norm = normalize_username(username);
        self.owner == norm || self.collaborators.iter().any(|c| c == &norm)
    }

    /// Return all authorized usernames (`owner` followed by `collaborators`).
    #[must_use]
    pub fn all_members(&self) -> Vec<String> {
        let mut members = Vec::with_capacity(1 + self.collaborators.len());
        if !self.owner.is_empty() {
            members.push(self.owner.clone());
        }
        for collab in &self.collaborators {
            if !members.contains(collab) {
                members.push(collab.clone());
            }
        }
        members
    }

    /// Return whether `username` currently has at least one wrapped key in `recipient_keys`.
    #[must_use]
    pub fn has_wrapped_key_for(&self, username: &str) -> bool {
        let norm = normalize_username(username);
        self.recipient_keys.iter().any(|rk| rk.username == norm)
    }

    /// Attempt to unwrap the [`ProjectKey`] using any of the provided local keypairs.
    ///
    /// # Errors
    /// Returns [`CollabError::Unauthorized`] if none of `local_keys` can unlock this envelope.
    pub fn unwrap_project_key(
        &self,
        local_keys: &[SshEd25519KeyPair],
    ) -> Result<ProjectKey, CollabError> {
        // First try matching by exact public key fingerprint
        for keypair in local_keys {
            let fp = keypair.public_key().fingerprint();
            for entry in &self.recipient_keys {
                if entry.fingerprint == fp
                    && let Ok(project_key) = entry.unwrap(keypair)
                {
                    return Ok(project_key);
                }
            }
        }
        // Fallback: try all entries against all local keys
        for keypair in local_keys {
            for entry in &self.recipient_keys {
                if let Ok(project_key) = entry.unwrap(keypair) {
                    return Ok(project_key);
                }
            }
        }
        Err(CollabError::Unauthorized(self.project_name.clone()))
    }

    /// Add a collaborator and wrap `project_key` for their `ssh-ed25519` public keys.
    ///
    /// If `public_keys` is empty, the collaborator is recorded in `collaborators` so
    /// their key can be wrapped on the next sync when their public key becomes available.
    ///
    /// # Errors
    /// Returns [`CollabError`] if `username` is invalid or key wrapping fails.
    pub fn add_collaborator(
        &mut self,
        username: &str,
        public_keys: &[SshEd25519PublicKey],
        project_key: &ProjectKey,
    ) -> Result<(), CollabError> {
        let norm = validate_github_username(username)?;
        if norm != self.owner && !self.collaborators.contains(&norm) {
            self.collaborators.push(norm.clone());
            self.collaborators.sort();
        }
        self.wrap_keys_for_user(&norm, public_keys, project_key)?;
        self.updated_at = Utc::now().to_rfc3339();
        Ok(())
    }

    /// Ensure `public_keys` for `username` are wrapped in `recipient_keys` and cached.
    ///
    /// # Errors
    /// Returns [`CollabError`] if key wrapping fails.
    pub fn wrap_keys_for_user(
        &mut self,
        username: &str,
        public_keys: &[SshEd25519PublicKey],
        project_key: &ProjectKey,
    ) -> Result<(), CollabError> {
        let norm = normalize_username(username);
        if public_keys.is_empty() {
            return Ok(());
        }
        let cache_entry = self.public_key_cache.entry(norm.clone()).or_default();
        for pubkey in public_keys {
            let fp = pubkey.fingerprint();
            if !self
                .recipient_keys
                .iter()
                .any(|rk| rk.username == norm && rk.fingerprint == fp)
            {
                let wrapped = WrappedProjectKey::wrap(project_key, &norm, pubkey)?;
                self.recipient_keys.push(wrapped);
            }
            let line = pubkey.to_openssh_line();
            if !cache_entry.contains(&line) {
                cache_entry.push(line);
            }
        }
        Ok(())
    }

    /// Remove a collaborator and rotate the [`ProjectKey`] to a fresh random key.
    ///
    /// All remaining members (`owner` + remaining `collaborators`) have the new key
    /// wrapped using their cached (and `extra_keys`) public keys, while the removed
    /// collaborator is purged from `collaborators`, `recipient_keys`, and `public_key_cache`.
    ///
    /// # Errors
    /// Returns [`CollabError`] if key generation or re-wrapping fails.
    pub fn remove_collaborator_and_rotate(
        &mut self,
        username: &str,
        extra_keys_for_owner: &[SshEd25519PublicKey],
    ) -> Result<ProjectKey, CollabError> {
        let norm = normalize_username(username);
        self.collaborators.retain(|c| c != &norm);
        self.public_key_cache.remove(&norm);

        let new_project_key = ProjectKey::generate()?;
        self.key_generation = self.key_generation.saturating_add(1);
        self.recipient_keys.clear();

        // Ensure owner's extra keys are in the cache
        if !extra_keys_for_owner.is_empty() && !self.owner.is_empty() {
            let owner_cache = self
                .public_key_cache
                .entry(self.owner.clone())
                .or_default();
            for key in extra_keys_for_owner {
                let line = key.to_openssh_line();
                if !owner_cache.contains(&line) {
                    owner_cache.push(line);
                }
            }
        }

        let members = self.all_members();
        for member in members {
            if let Some(lines) = self.public_key_cache.get(&member).cloned() {
                let parsed: Vec<SshEd25519PublicKey> = lines
                    .iter()
                    .filter_map(|l| SshEd25519PublicKey::parse_openssh_line(l).ok())
                    .collect();
                self.wrap_keys_for_user(&member, &parsed, &new_project_key)?;
            }
        }

        self.updated_at = Utc::now().to_rfc3339();
        Ok(new_project_key)
    }
}

/// A single file entry inside a serialized project archive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundledProjectFile {
    /// Forward-slash relative path inside the project (e.g. `sections/intro.tex`).
    pub relative_path: String,
    /// Base64-encoded file contents.
    pub content_b64: String,
    /// Hex-encoded SHA-256 of the raw file bytes.
    pub sha256: String,
}

/// Plaintext payload of a project archive before gzip + AEAD encryption.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectArchive {
    /// Archive format version.
    pub version: u32,
    /// Project name.
    pub project_name: String,
    /// Compiler (`"typst"` or `"latex"`).
    pub compiler: String,
    /// Key generation used to encrypt this archive.
    pub key_generation: u32,
    /// ISO 8601 timestamp when packaged.
    pub packaged_at: String,
    /// Files included in the project.
    pub files: Vec<BundledProjectFile>,
}

/// Package and encrypt all source/asset files in `project_root` using `project_key`.
///
/// Build artifacts (`main.pdf`, `.aux`, `.log`, `.fdb_latexmk`, `.fls`, `.out`, `.bbl`, `.blg`),
/// `.git`, and local key files are excluded automatically.
///
/// # Errors
/// Returns [`CollabError`] if reading project files, compressing, or encrypting fails.
pub fn encrypt_project_directory(
    project_root: &Path,
    project_name: &str,
    compiler: &str,
    key_generation: u32,
    project_key: &ProjectKey,
) -> Result<Vec<u8>, CollabError> {
    let mut files = Vec::new();

    for entry in WalkDir::new(project_root)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| !is_excluded_project_entry(e.path(), project_root))
    {
        let entry = entry.map_err(std::io::Error::other)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(project_root)
            .map_err(|e| CollabError::InvalidBundlePath(e.to_string()))?;
        let rel_str = normalize_relative_path(rel)?;
        let raw_bytes = fs::read(entry.path())?;
        let sha256 = hex_encode(&Sha256::digest(&raw_bytes));
        files.push(BundledProjectFile {
            relative_path: rel_str,
            content_b64: BASE64_STANDARD.encode(&raw_bytes),
            sha256,
        });
    }

    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    let archive = ProjectArchive {
        version: 1,
        project_name: project_name.to_string(),
        compiler: compiler.to_string(),
        key_generation,
        packaged_at: Utc::now().to_rfc3339(),
        files,
    };

    let json_bytes = serde_json::to_vec(&archive)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&json_bytes)?;
    let compressed = encoder.finish()?;

    let rng = SystemRandom::new();
    let mut nonce_bytes = [0u8; 12];
    rng.fill(&mut nonce_bytes)
        .map_err(|_| CollabError::Crypto("failed to generate bundle nonce".into()))?;

    let unbound = UnboundKey::new(&CHACHA20_POLY1305, project_key.as_bytes())
        .map_err(|_| CollabError::Crypto("failed to initialize bundle cipher".into()))?;
    let key = LessSafeKey::new(unbound);

    let aad = bundle_aad(project_name, key_generation);
    let mut ciphertext = compressed;
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce_bytes),
        Aad::from(aad.as_bytes()),
        &mut ciphertext,
    )
    .map_err(|_| CollabError::Crypto("failed to encrypt project bundle".into()))?;

    let mut out = Vec::with_capacity(BUNDLE_MAGIC.len() + 4 + 12 + ciphertext.len());
    out.extend_from_slice(BUNDLE_MAGIC);
    out.extend_from_slice(&key_generation.to_be_bytes());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt an encrypted project bundle (`bundle.enc`) into a [`ProjectArchive`].
///
/// # Errors
/// Returns [`CollabError`] if authentication fails or the bundle is corrupted.
pub fn decrypt_project_bundle(
    encrypted_bundle: &[u8],
    project_name: &str,
    project_key: &ProjectKey,
) -> Result<ProjectArchive, CollabError> {
    let header_len = BUNDLE_MAGIC.len() + 4 + 12;
    if encrypted_bundle.len() < header_len + 16 || !encrypted_bundle.starts_with(BUNDLE_MAGIC) {
        return Err(CollabError::Crypto("invalid encrypted bundle header".into()));
    }
    let gen_offset = BUNDLE_MAGIC.len();
    let key_generation = u32::from_be_bytes(
        encrypted_bundle[gen_offset..gen_offset + 4]
            .try_into()
            .map_err(|_| CollabError::Crypto("invalid key generation header".into()))?,
    );
    let nonce_offset = gen_offset + 4;
    let nonce_bytes: [u8; 12] = encrypted_bundle[nonce_offset..nonce_offset + 12]
        .try_into()
        .map_err(|_| CollabError::Crypto("invalid bundle nonce".into()))?;

    let mut ciphertext = encrypted_bundle[header_len..].to_vec();
    let unbound = UnboundKey::new(&CHACHA20_POLY1305, project_key.as_bytes())
        .map_err(|_| CollabError::Crypto("failed to initialize bundle cipher".into()))?;
    let key = LessSafeKey::new(unbound);

    let aad = bundle_aad(project_name, key_generation);
    let compressed = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(aad.as_bytes()),
            &mut ciphertext,
        )
        .map_err(|_| {
            CollabError::Crypto(
                "bundle decryption failed: invalid project key or tampered data".into(),
            )
        })?;

    let mut decoder = GzDecoder::new(&*compressed);
    let mut json_bytes = Vec::new();
    decoder.read_to_end(&mut json_bytes)?;

    let archive: ProjectArchive = serde_json::from_slice(&json_bytes)?;
    Ok(archive)
}

/// Unpack a decrypted [`ProjectArchive`] into `project_root`.
///
/// Validates all relative paths to ensure no path traversal outside `project_root`.
///
/// # Errors
/// Returns [`CollabError`] if any path is unsafe or file writing fails.
pub fn unpack_project_archive(
    archive: &ProjectArchive,
    project_root: &Path,
) -> Result<(), CollabError> {
    fs::create_dir_all(project_root)?;
    fs::create_dir_all(project_root.join("figures"))?;
    fs::create_dir_all(project_root.join("sections"))?;

    for file in &archive.files {
        let rel_path = validate_safe_relative_path(&file.relative_path)?;
        let target = project_root.join(&rel_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw_bytes = BASE64_STANDARD.decode(&file.content_b64)?;
        fs::write(&target, &raw_bytes)?;
    }
    Ok(())
}

/// Read the local [`ProjectKeyEnvelope`] from `<project_root>/.papr-collab.json` if present.
///
/// # Errors
/// Returns [`CollabError`] if the file exists but cannot be parsed.
pub fn load_local_envelope(
    project_root: &Path,
) -> Result<Option<ProjectKeyEnvelope>, CollabError> {
    let path = project_root.join(LOCAL_COLLAB_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)?;
    let envelope: ProjectKeyEnvelope = serde_json::from_str(&text)?;
    Ok(Some(envelope))
}

/// Save `envelope` to `<project_root>/.papr-collab.json` and optionally cache `project_key`.
///
/// # Errors
/// Returns [`CollabError`] if writing fails.
pub fn save_local_envelope(
    project_root: &Path,
    envelope: &ProjectKeyEnvelope,
    project_key: Option<&ProjectKey>,
) -> Result<(), CollabError> {
    fs::create_dir_all(project_root)?;
    let path = project_root.join(LOCAL_COLLAB_FILE);
    fs::write(path, serde_json::to_vec_pretty(envelope)?)?;

    if let Some(key) = project_key {
        let key_path = project_root.join(LOCAL_PROJECT_KEY_FILE);
        fs::write(&key_path, key.to_base64())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600));
        }
    }
    Ok(())
}

/// Load or unwrap the [`ProjectKey`] for `project_root`, or initialize a new envelope and key
/// if the project does not have one yet.
///
/// # Errors
/// Returns [`CollabError`] if the user is unauthorized or initialization fails.
pub fn ensure_local_project_collab(
    project_root: &Path,
    project_name: &str,
    compiler: &str,
    owner: &str,
    local_keys: &[SshEd25519KeyPair],
) -> Result<(ProjectKeyEnvelope, ProjectKey), CollabError> {
    let owner_pubkeys: Vec<SshEd25519PublicKey> =
        local_keys.iter().map(|k| k.public_key().clone()).collect();

    if let Some(mut envelope) = load_local_envelope(project_root)? {
        // First try unwrapping with local SSH keys
        let project_key = if let Ok(k) = envelope.unwrap_project_key(local_keys) {
            k
        } else {
            // Fallback to cached local project key if this machine created it
            let key_path = project_root.join(LOCAL_PROJECT_KEY_FILE);
            if key_path.is_file() {
                let encoded = fs::read_to_string(key_path)?;
                ProjectKey::from_base64(&encoded)?
            } else {
                return Err(CollabError::Unauthorized(project_name.to_string()));
            }
        };

        if envelope.owner.is_empty() && !owner.trim().is_empty() {
            envelope.owner = normalize_username(owner);
        }
        if ! envelope.owner.is_empty() {
            let owner_name = envelope.owner.clone();
            envelope.wrap_keys_for_user(&owner_name, &owner_pubkeys, &project_key)?;
        }
        save_local_envelope(project_root, &envelope, Some(&project_key))?;
        return Ok((envelope, project_key));
    }

    let project_key = ProjectKey::generate()?;
    let detected_compiler = if project_root.join("main.typ").is_file() {
        "typst"
    } else if project_root.join("main.tex").is_file() {
        "latex"
    } else {
        compiler
    };
    let envelope = ProjectKeyEnvelope::new(
        project_name,
        detected_compiler,
        owner,
        &owner_pubkeys,
        &project_key,
    )?;
    save_local_envelope(project_root, &envelope, Some(&project_key))?;
    Ok((envelope, project_key))
}

/// Validate and normalize a GitHub username to lowercase.
///
/// # Errors
/// Returns [`CollabError::InvalidUsername`] if empty or containing invalid characters.
pub fn validate_github_username(username: &str) -> Result<String, CollabError> {
    let trimmed = username.trim().trim_start_matches('@').trim();
    if trimmed.is_empty()
        || trimmed.len() > 39
        || trimmed.starts_with('-')
        || trimmed.ends_with('-')
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(CollabError::InvalidUsername(username.to_string()));
    }
    Ok(trimmed.to_ascii_lowercase())
}

fn normalize_username(username: &str) -> String {
    username
        .trim()
        .trim_start_matches('@')
        .trim()
        .to_ascii_lowercase()
}

fn bundle_aad(project_name: &str, key_generation: u32) -> String {
    format!("papr-project-bundle-v1:{project_name}:gen{key_generation}")
}

fn is_excluded_project_entry(path: &Path, project_root: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    if matches!(
        name,
        ".git" | LOCAL_COLLAB_FILE | LOCAL_PROJECT_KEY_FILE | "main.pdf"
    ) {
        return true;
    }
    if path.parent() == Some(project_root) && name.starts_with(".papr") {
        return true;
    }
    if path.is_file()
        && let Some(ext) = path.extension().and_then(|e| e.to_str())
        && matches!(
            ext,
            "aux" | "bbl" | "blg" | "fdb_latexmk" | "fls" | "log" | "out" | "synctex"
        )
    {
        return true;
    }
    false
}

fn normalize_relative_path(rel: &Path) -> Result<String, CollabError> {
    let mut parts = Vec::new();
    for comp in rel.components() {
        match comp {
            Component::Normal(os) => {
                let s = os.to_str().ok_or_else(|| {
                    CollabError::InvalidBundlePath("non-UTF-8 path component".into())
                })?;
                parts.push(s);
            }
            _ => {
                return Err(CollabError::InvalidBundlePath(
                    rel.display().to_string(),
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(CollabError::InvalidBundlePath("empty relative path".into()));
    }
    Ok(parts.join("/"))
}

fn validate_safe_relative_path(rel: &str) -> Result<PathBuf, CollabError> {
    let path = Path::new(rel);
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(CollabError::InvalidBundlePath(rel.to_string()));
    }
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(part) => {
                let s = part.to_string_lossy();
                if s == ".git" || s == LOCAL_COLLAB_FILE || s == LOCAL_PROJECT_KEY_FILE {
                    return Err(CollabError::InvalidBundlePath(rel.to_string()));
                }
                out.push(part);
            }
            _ => return Err(CollabError::InvalidBundlePath(rel.to_string())),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(CollabError::InvalidBundlePath(rel.to_string()));
    }
    Ok(out)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0x0f)]));
    }
    out
}

fn read_ssh_u32(buf: &[u8], cursor: &mut usize) -> Result<u32, CollabError> {
    let end = cursor
        .checked_add(4)
        .ok_or_else(|| CollabError::InvalidSshKey("buffer overflow reading u32".into()))?;
    let slice = buf
        .get(*cursor..end)
        .ok_or_else(|| CollabError::InvalidSshKey("unexpected EOF reading u32".into()))?;
    *cursor = end;
    let bytes: [u8; 4] = slice
        .try_into()
        .map_err(|_| CollabError::InvalidSshKey("invalid u32 slice".into()))?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_ssh_string<'a>(buf: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], CollabError> {
    let len = usize::try_from(read_ssh_u32(buf, cursor)?)
        .map_err(|_| CollabError::InvalidSshKey("string length overflow".into()))?;
    let end = cursor
        .checked_add(len)
        .ok_or_else(|| CollabError::InvalidSshKey("buffer overflow reading string".into()))?;
    let slice = buf
        .get(*cursor..end)
        .ok_or_else(|| CollabError::InvalidSshKey("unexpected EOF reading string".into()))?;
    *cursor = end;
    Ok(slice)
}

fn write_ssh_u32(out: &mut Vec<u8>, val: u32) {
    out.extend_from_slice(&val.to_be_bytes());
}

fn write_ssh_string(out: &mut Vec<u8>, data: &[u8]) {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    write_ssh_u32(out, len);
    out.extend_from_slice(data);
}

struct WrappingKeyLen;

impl KeyType for WrappingKeyLen {
    fn len(&self) -> usize {
        32
    }
}

fn derive_wrapping_key(
    shared_secret: &[u8; 32],
    ephemeral_pub: &[u8; 32],
    recipient_pub: &[u8; 32],
) -> Result<[u8; 32], CollabError> {
    let mut salt_bytes = [0u8; 64];
    salt_bytes[..32].copy_from_slice(ephemeral_pub);
    salt_bytes[32..].copy_from_slice(recipient_pub);

    let salt = Salt::new(HKDF_SHA256, &salt_bytes);
    let prk = salt.extract(shared_secret);
    let info = [KEY_WRAP_INFO];
    let okm = prk
        .expand(&info, WrappingKeyLen)
        .map_err(|_| CollabError::Crypto("HKDF expand failed".into()))?;
    let mut out = [0u8; 32];
    okm.fill(&mut out)
        .map_err(|_| CollabError::Crypto("HKDF fill failed".into()))?;
    Ok(out)
}

// ============================================================================
// Pure-Rust RFC 7748 X25519 & Ed25519 -> X25519 Birational Map (GF(2^255 - 19))
// ============================================================================

type FieldElem = [i64; 16];

const GF_ONE: FieldElem = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
const GF_121665: FieldElem = [0xdb41, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn clamp_x25519_scalar(scalar: &mut [u8; 32]) {
    scalar[0] &= 0xf8;
    scalar[31] &= 0x7f;
    scalar[31] |= 0x40;
}

fn unpack25519(bytes: &[u8; 32]) -> FieldElem {
    let mut out = [0i64; 16];
    for i in 0..16 {
        out[i] = i64::from(bytes[2 * i]) | (i64::from(bytes[2 * i + 1]) << 8);
    }
    out[15] &= 0x7fff;
    out
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pack25519(elem: &FieldElem) -> [u8; 32] {
    let mut t = *elem;
    car25519(&mut t);
    car25519(&mut t);
    car25519(&mut t);
    let mut m = [0i64; 16];
    for _ in 0..2 {
        m[0] = t[0] - 0xffed;
        for i in 1..15 {
            m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        let b = (m[15] >> 16) & 1;
        m[14] &= 0xffff;
        sel25519(&mut t, &mut m, 1 - b);
    }
    let mut out = [0u8; 32];
    for i in 0..16 {
        out[2 * i] = (t[i] & 0xff) as u8;
        out[2 * i + 1] = ((t[i] >> 8) & 0xff) as u8;
    }
    out
}

fn car25519(o: &mut FieldElem) {
    for i in 0..16 {
        o[i] += 1 << 16;
        let c = o[i] >> 16;
        if i < 15 {
            o[i + 1] += c - 1;
        } else {
            o[0] += 38 * (c - 1);
        }
        o[i] -= c << 16;
    }
}

#[allow(clippy::many_single_char_names)]
fn sel25519(p: &mut FieldElem, q: &mut FieldElem, b: i64) {
    let c = !(b - 1);
    for i in 0..16 {
        let t = c & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

fn fe_add(a: &FieldElem, b: &FieldElem) -> FieldElem {
    let mut out = [0i64; 16];
    for i in 0..16 {
        out[i] = a[i] + b[i];
    }
    out
}

fn fe_sub(a: &FieldElem, b: &FieldElem) -> FieldElem {
    let mut out = [0i64; 16];
    for i in 0..16 {
        out[i] = a[i] - b[i];
    }
    out
}

fn fe_mul(a: &FieldElem, b: &FieldElem) -> FieldElem {
    let mut t = [0i64; 31];
    for i in 0..16 {
        for j in 0..16 {
            t[i + j] += a[i] * b[j];
        }
    }
    for i in 0..15 {
        t[i] += 38 * t[i + 16];
    }
    let mut out = [0i64; 16];
    out.copy_from_slice(&t[..16]);
    car25519(&mut out);
    car25519(&mut out);
    out
}

fn fe_sq(a: &FieldElem) -> FieldElem {
    fe_mul(a, a)
}

fn inv25519(i: &FieldElem) -> FieldElem {
    let mut c = *i;
    for a in (0..=253).rev() {
        c = fe_sq(&c);
        if a != 2 && a != 4 {
            c = fe_mul(&c, i);
        }
    }
    c
}

/// Convert a 32-byte Ed25519 public key ($y$ coordinate) into a 32-byte X25519 public key ($u$ coordinate):
/// $u = (1 + y) \cdot (1 - y)^{-1} \pmod{2^{255} - 19}$.
fn ed25519_pubkey_to_x25519(ed25519_bytes: &[u8; 32]) -> Result<[u8; 32], CollabError> {
    let y = unpack25519(ed25519_bytes);
    let num = fe_add(&GF_ONE, &y);
    let den = fe_sub(&GF_ONE, &y);
    let den_bytes = pack25519(&den);
    if den_bytes == [0u8; 32] {
        return Err(CollabError::InvalidSshKey(
            "identity Ed25519 point cannot be converted to X25519".into(),
        ));
    }
    let den_inv = inv25519(&den);
    let u = fe_mul(&num, &den_inv);
    let u_bytes = pack25519(&u);
    if u_bytes == [0u8; 32] {
        return Err(CollabError::InvalidSshKey(
            "low-order Ed25519 public key point rejected".into(),
        ));
    }
    Ok(u_bytes)
}

/// Compute RFC 7748 `X25519(scalar, basepoint_9)`.
#[must_use]
pub fn x25519_base(scalar: &[u8; 32]) -> [u8; 32] {
    let mut nine = [0u8; 32];
    nine[0] = 9;
    x25519_raw(scalar, &nine)
}

/// Compute RFC 7748 `X25519(scalar, point)`, rejecting all-zero low-order shared secrets.
///
/// # Errors
/// Returns [`CollabError::Crypto`] if the shared secret is all zeros.
pub fn x25519(scalar: &[u8; 32], point: &[u8; 32]) -> Result<[u8; 32], CollabError> {
    let shared = x25519_raw(scalar, point);
    if shared == [0u8; 32] {
        return Err(CollabError::Crypto(
            "invalid low-order X25519 peer public key".into(),
        ));
    }
    Ok(shared)
}

#[allow(clippy::many_single_char_names)]
fn x25519_raw(scalar: &[u8; 32], point: &[u8; 32]) -> [u8; 32] {
    let mut z = *scalar;
    clamp_x25519_scalar(&mut z);

    let x = unpack25519(point);
    let mut a = GF_ONE;
    let mut b = x;
    let mut c = [0i64; 16];
    let mut d = GF_ONE;

    for i in (0..=254).rev() {
        let bit = i64::from((z[i >> 3] >> (i & 7)) & 1);
        sel25519(&mut a, &mut b, bit);
        sel25519(&mut c, &mut d, bit);

        let e = fe_add(&a, &c);
        a = fe_sub(&a, &c);
        c = fe_add(&b, &d);
        b = fe_sub(&b, &d);

        d = fe_sq(&e);
        let f = fe_sq(&a);
        a = fe_mul(&c, &a);
        c = fe_mul(&b, &e);

        let e2 = fe_add(&a, &c);
        a = fe_sub(&a, &c);
        b = fe_sq(&a);
        c = fe_sub(&d, &f);

        a = fe_mul(&c, &GF_121665);
        a = fe_add(&a, &d);
        c = fe_mul(&c, &a);
        a = fe_mul(&d, &f);
        d = fe_mul(&b, &x);
        b = fe_sq(&e2);

        sel25519(&mut a, &mut b, bit);
        sel25519(&mut c, &mut d, bit);
    }

    let c_inv = inv25519(&c);
    let res = fe_mul(&a, &c_inv);
    pack25519(&res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7748_x25519_test_vector_matches() {
        let alice_priv: [u8; 32] = [
            0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d, 0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2,
            0x66, 0x45, 0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a, 0xb1, 0x77, 0xfb, 0xa5,
            0x1d, 0xb9, 0x2c, 0x2a,
        ];
        let expected_alice_pub: [u8; 32] = [
            0x85, 0x20, 0xf0, 0x09, 0x89, 0x30, 0xa7, 0x54, 0x74, 0x8b, 0x7d, 0xdc, 0xb4, 0x3e,
            0xf7, 0x5a, 0x0d, 0xbf, 0x3a, 0x0d, 0x26, 0x38, 0x1a, 0xf4, 0xeb, 0xa4, 0xa9, 0x8e,
            0xaa, 0x9b, 0x4e, 0x6a,
        ];
        let bob_priv: [u8; 32] = [
            0x5d, 0xab, 0x08, 0x7e, 0x62, 0x4a, 0x8a, 0x4b, 0x79, 0xe1, 0x7f, 0x8b, 0x83, 0x80,
            0x0e, 0xe6, 0x6f, 0x3b, 0xb1, 0x29, 0x26, 0x18, 0xb6, 0xfd, 0x1c, 0x2f, 0x8b, 0x27,
            0xff, 0x88, 0xe0, 0xeb,
        ];
        let expected_shared: [u8; 32] = [
            0x4a, 0x5d, 0x9d, 0x5b, 0xa4, 0xce, 0x2d, 0xe1, 0x72, 0x8e, 0x3b, 0xf4, 0x80, 0x35,
            0x0f, 0x25, 0xe0, 0x7e, 0x21, 0xc9, 0x47, 0xd1, 0x9e, 0x33, 0x76, 0xf0, 0x9b, 0x3c,
            0x1e, 0x16, 0x17, 0x42,
        ];

        let alice_pub = x25519_base(&alice_priv);
        assert_eq!(alice_pub, expected_alice_pub);

        let bob_pub = x25519_base(&bob_priv);
        let shared_ab = x25519_raw(&alice_priv, &bob_pub);
        let shared_ba = x25519_raw(&bob_priv, &alice_pub);
        assert_eq!(shared_ab, expected_shared);
        assert_eq!(shared_ba, expected_shared);
    }

    #[test]
    fn ed25519_birational_map_matches_scalar_base_multiplication() -> Result<(), CollabError> {
        let pair = SshEd25519KeyPair::generate(Some("alice@papr".into()))?;
        let scalar = pair.to_x25519_scalar();
        let x_from_scalar = x25519_base(&scalar);
        let x_from_ed_pub = pair.public_key().to_x25519_bytes()?;
        assert_eq!(x_from_scalar, x_from_ed_pub);
        Ok(())
    }

    #[test]
    fn openssh_keypair_roundtrip_and_multi_collaborator_revocation() -> Result<(), CollabError> {
        let alice = SshEd25519KeyPair::generate(Some("alice".into()))?;
        let bob = SshEd25519KeyPair::generate(Some("bob".into()))?;
        let charlie = SshEd25519KeyPair::generate(Some("charlie".into()))?;

        // Verify OpenSSH private & public key PEM/line serialization round-trip
        let pem = alice.to_openssh_private_key_pem();
        let parsed_alice = SshEd25519KeyPair::parse_openssh_private_key(&pem)?;
        assert_eq!(
            parsed_alice.public_key().fingerprint(),
            alice.public_key().fingerprint()
        );

        let pub_line = bob.public_key().to_openssh_line();
        let parsed_bob_pub = SshEd25519PublicKey::parse_openssh_line(&pub_line)?;
        assert_eq!(parsed_bob_pub, *bob.public_key());

        // Create envelope for Alice and add Bob
        let initial_key = ProjectKey::generate()?;
        let mut envelope = ProjectKeyEnvelope::new(
            "quantum-optics",
            "typst",
            "alice",
            std::slice::from_ref(alice.public_key()),
            &initial_key,
        )?;
        envelope.add_collaborator("bob", &[parsed_bob_pub], &initial_key)?;

        // Alice and Bob can both unwrap the ProjectKey
        assert_eq!(
            envelope.unwrap_project_key(std::slice::from_ref(&alice))?,
            initial_key
        );
        assert_eq!(
            envelope.unwrap_project_key(std::slice::from_ref(&bob))?,
            initial_key
        );
        // Charlie is unauthorized!
        assert!(matches!(
            envelope.unwrap_project_key(std::slice::from_ref(&charlie)),
            Err(CollabError::Unauthorized(_))
        ));

        // Remove Bob -> rotates ProjectKey to generation 2
        let rotated_key =
            envelope.remove_collaborator_and_rotate("bob", std::slice::from_ref(alice.public_key()))?;
        assert_eq!(envelope.key_generation, 2);
        assert_ne!(rotated_key, initial_key);

        // Alice unlocks the new key; Bob is now locked out!
        assert_eq!(
            envelope.unwrap_project_key(std::slice::from_ref(&alice))?,
            rotated_key
        );
        assert!(matches!(
            envelope.unwrap_project_key(std::slice::from_ref(&bob)),
            Err(CollabError::Unauthorized(_))
        ));
        Ok(())
    }

    #[test]
    fn project_bundle_encrypts_and_unpacks_cleanly() -> Result<(), Box<dyn std::error::Error>> {
        let temp_src = std::env::temp_dir().join(format!(
            "papr-collab-src-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let temp_dst = std::env::temp_dir().join(format!(
            "papr-collab-dst-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir_all(temp_src.join("sections"))?;
        fs::write(temp_src.join("main.typ"), "= Hello Collab\n")?;
        fs::write(temp_src.join("sections/intro.typ"), "Some intro text.\n")?;
        // Build artifacts should be excluded
        fs::write(temp_src.join("main.pdf"), b"%PDF-1.4 artifact")?;
        fs::write(temp_src.join("main.log"), "compiler log")?;

        let key = ProjectKey::generate()?;
        let wrong_key = ProjectKey::generate()?;

        let encrypted =
            encrypt_project_directory(&temp_src, "collab-test", "typst", 1, &key)?;
        // Wrong key cannot decrypt
        assert!(decrypt_project_bundle(&encrypted, "collab-test", &wrong_key).is_err());

        // Right key decrypts and unpacks
        let archive = decrypt_project_bundle(&encrypted, "collab-test", &key)?;
        assert_eq!(archive.files.len(), 2);
        unpack_project_archive(&archive, &temp_dst)?;

        assert_eq!(
            fs::read_to_string(temp_dst.join("main.typ"))?,
            "= Hello Collab\n"
        );
        assert_eq!(
            fs::read_to_string(temp_dst.join("sections/intro.typ"))?,
            "Some intro text.\n"
        );
        assert!(!temp_dst.join("main.pdf").exists());

        fs::remove_dir_all(temp_src)?;
        fs::remove_dir_all(temp_dst)?;
        Ok(())
    }
}
