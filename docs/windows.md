# Windows Setup

Concise setup notes for Windows users. Visual Studio Build Tools are only needed when building Papr from source; MiKTeX, `latexmk`, and Strawberry Perl are the Windows LaTeX workspace prerequisites.

## Visual Studio Build Tools

Build Tools are needed only for source builds on Windows because Papr and some of its native dependencies require the MSVC toolchain, linker, and C/C++ headers.

1. Install Visual Studio Build Tools 2022 and select the `Desktop development with C++` workload.
2. Make sure the C++ build tools, MSVC v143 toolset, and a Windows SDK are included.

Command-line installation with `winget`:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --force --override "--wait --passive --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows10SDK"
```

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --force --override "--wait --passive --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.22621"
```

## MiKTeX

Download: [MiKTeX for Windows](https://miktex.org/download)

1. Download the Windows installer from the MiKTeX download page.
2. Run the installer.
3. Choose `Install for all users` if multiple users will use the system.
4. Select `Install missing packages on-the-fly` so MiKTeX can install missing packages automatically.
5. Complete the installation.
6. Open **MiKTeX Console** to verify the install.

## latexmk

1. Open **MiKTeX Console**.
2. Search for `latexmk` in the Packages view and install it.
3. Verify it works:

```powershell
latexmk --version
```

## Add MiKTeX to PATH

Default MiKTeX binaries path:

```text
C:\Program Files\MiKTeX\miktex\bin\x64
```

If `latexmk` is not recognized in the command line:

1. Open **This PC** > **Properties** > **Advanced system settings** > **Environment Variables**.
2. Edit `Path`.
3. Add the directory containing `latexmk.exe`.
4. Open a new Command Prompt or PowerShell window.

## Strawberry Perl

Download: [Strawberry Perl](https://strawberryperl.com/)

1. Download the 64-bit MSI installer from the Strawberry Perl downloads page, for example `strawberry-perl-5.42.2.1-64bit.msi`.
2. Run the MSI installer.
3. Accept the license agreement and keep the default install directory unless you need a different one.
4. Enable the PATH option if the installer offers it.
5. Finish the installation.
6. Verify the install:

```powershell
perl -v
```
