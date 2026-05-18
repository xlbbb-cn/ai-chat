# Tauri + React + Typescript

This template should help get you started developing with Tauri, React and Typescript in Vite.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

## Windows GNU Toolchain Notes

If your Rust host is `x86_64-pc-windows-gnu`, you may need `dlltool.exe` and other GNU toolchain binaries to build native dependencies and install `tauri-cli`.

- Install MSYS2 from https://www.msys2.org/
- Open the MSYS2 MinGW64 shell and run:
  ```sh
  pacman -Syu
  pacman -S --needed base-devel mingw-w64-x86_64-toolchain
  ```
- Add `C:\msys64\mingw64\bin` to your Windows `PATH` and restart your terminal.
- Retry:
  ```sh
  cargo install tauri-cli --version "^2"
  ```

If you prefer MSVC instead of GNU, install Visual Studio Build Tools with the C++ workload and use the MSVC toolchain:

```powershell
rustup default stable-x86_64-pc-windows-msvc
```

## Windows WebView2 / Tauri build note

If you get a linker error such as `export ordinal too large` while compiling `webview2-com` or `rustgui_lib` with `x86_64-w64-mingw32-gcc`, this is usually a limitation of the GNU/Mingw linker when building WebView2-based Tauri apps.

- Prefer the MSVC toolchain on Windows for Tauri desktop builds.
- Make sure your `cargo` and `rustc` are the ones managed by `rustup`, not a separate Chocolatey/MSYS2 Rust installation.
- Confirm by running:
  ```powershell
  rustc --version --verbose
  cargo --version
  rustup toolchain list
  ```
- If you see `x86_64-pc-windows-gnu` or commands resolved from `C:\Applications\choco\bin`, fix your PATH so `C:\Users\leonard\.cargo\bin` comes first.
