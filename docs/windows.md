# Windows Setup

Concise setup notes for Windows users. Visual Studio Build Tools are only needed when building Papr from source; MiKTeX, `latexmk`, and Strawberry Perl are the Windows LaTeX workspace dependencies.

## Visual Studio Build Tools

Build Tools are needed only for source builds on Windows because Papr and some of its native dependencies require the MSVC toolchain, linker, and C/C++ headers.

1. Install Visual Studio Build Tools 2022 and select the `Desktop development with C++` workload.
2. Make sure the C++ build tools, MSVC v143 toolset, and a Windows SDK are included.

Windows SDK options:

```powershell
winget install --id Microsoft.WindowsSDK.10.0.19041 --source winget
```

```powershell
winget install --id Microsoft.WindowsSDK.10.0.22621 --source winget
```

## MiKTeX

Download: [MiKTeX for Windows](https://miktex.org/download)

1. Download the Basic MiKTeX Installer.
2. Run the installer and keep the default private install unless you need a shared setup.
3. Leave `Install missing packages on-the-fly` enabled so missing TeX packages are fetched automatically.
4. Finish the setup, then open **MiKTeX Console**.
5. In MiKTeX Console, use `Updates` -> `Check for Updates` to verify the installation.

## latexmk

1. Open **MiKTeX Console**.
2. Search for `latexmk` in the Packages view and install it.
3. Verify it works:

```powershell
latexmk -v
```

## Add MiKTeX to PATH

Default per-user MiKTeX binaries path:

```text
%LOCALAPPDATA%\Programs\MiKTeX\miktex\bin\x64
```

1. Open **Environment Variables** in Windows.
2. Edit `Path` for your user account.
3. Add the MiKTeX `bin\x64` directory, then reopen your terminal.

## Strawberry Perl

Download: [Strawberry Perl](https://strawberryperl.com/)

1. Download the 64-bit MSI installer from the release page.
2. Run the MSI and keep the default installation options.
3. Leave the PATH option enabled if the installer offers it.
4. Verify the install:

```powershell
perl -v
```
