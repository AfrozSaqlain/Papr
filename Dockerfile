# syntax=docker/dockerfile:1

# The current locked dependency graph is not compatible with Rust 1.85.1.
# `1-bookworm` tracks the current stable Rust 1.x release on Debian Bookworm.
FROM rust:1-bookworm AS builder

WORKDIR /src

# Build requirements for crates that compile native code. SQLite itself is
# bundled by papr-core, so no runtime SQLite package is required.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        build-essential \
        pkg-config \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY . .
RUN cargo build --locked --release --bin papr


FROM debian:bookworm-slim AS runtime

ARG USERNAME=papr
ARG UID=1000
ARG GID=1000

ENV HOME=/home/${USERNAME} \
    XDG_CONFIG_HOME=/home/${USERNAME}/.config \
    XDG_DATA_HOME=/home/${USERNAME}/.local/share \
    RUST_BACKTRACE=1

# poppler-utils: internal viewer (pdftoppm), page counts (pdfinfo), and text
# metadata extraction (pdftotext).  wl-clipboard/xclip enable Linux clipboard
# integration. zathura plus its Poppler backend enables configured external
# viewing when a display socket is passed through.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        ca-certificates \
        poppler-utils \
        wl-clipboard \
        xclip \
        xdg-utils \
        zathura \
        zathura-pdf-poppler \
        fontconfig \
        fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid "${GID}" "${USERNAME}" \
    && useradd --uid "${UID}" --gid "${GID}" --create-home \
        --shell /bin/bash "${USERNAME}" \
    && mkdir -p "${XDG_CONFIG_HOME}/papr" "${XDG_DATA_HOME}/papr" \
    && chown -R "${UID}:${GID}" "${HOME}"

COPY --from=builder /src/target/release/papr /usr/local/bin/papr

USER ${USERNAME}
WORKDIR /home/${USERNAME}

# `papr` accepts subcommands (for example `paths` and `plugins`) supplied to
# `docker run` after the image name.
ENTRYPOINT ["papr"]
CMD []
