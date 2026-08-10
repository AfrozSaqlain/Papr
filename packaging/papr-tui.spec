Name:           papr-tui
Version:        0.1.1
Release:        1%{?dist}
Summary:        Keyboard-first terminal workspace for researchers

License:        MIT
URL:            https://github.com/AfrozSaqlain/Papr
Source0:        %{url}/archive/refs/tags/v%{version}/Papr-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  pkgconf-pkg-config
BuildRequires:  rust-packaging
BuildRequires:  sqlite-devel

# The application invokes these tools for optional PDF, LaTeX, clipboard, and
# desktop-integration features. They are kept as weak dependencies so the
# terminal application remains installable in minimal environments.
Recommends:     poppler-utils
Recommends:     xdg-utils
Recommends:     latexmk
Recommends:     wl-clipboard
Recommends:     xclip

%description
Papr is a keyboard-first terminal workspace for academic research. It
provides paper discovery, local PDF library management, reading, notes,
metadata enrichment, and LaTeX and embedded Typst project support in a terminal interface.

%prep
%autosetup -n Papr-%{version}

# Fedora builds use the system SQLite rather than rusqlite's bundled copy.
%cargo_prep

%generate_buildrequires
%cargo_generate_buildrequires --no-default-features -p papr-tui

%build
%cargo_build --no-default-features -p papr-tui

%install
install -Dpm0755 target/release/papr %{buildroot}%{_bindir}/papr
install -Dpm0644 LICENSE %{buildroot}%{_licensedir}/%{name}/LICENSE
install -Dpm0644 README.md %{buildroot}%{_docdir}/%{name}/README.md

%files
%license %{_licensedir}/%{name}/LICENSE
%doc %{_docdir}/%{name}/README.md
%{_bindir}/papr

%changelog
* Mon Aug 03 2026 Papr contributors <papr@example.invalid> - 0.1.1-1
- Initial Fedora COPR package
