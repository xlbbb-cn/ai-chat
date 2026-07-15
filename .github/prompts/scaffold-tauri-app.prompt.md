---
name: scaffold-tauri-app
description: "Scaffold a new Tauri 2.0 Rust GUI project in the current workspace. Runs tauri init, sets up frontend (Vite + React or Vanilla), and wires initial Tauri commands."
---

# Scaffold Tauri 2.0 App

Set up a new Tauri 2.0 desktop application in the current workspace.

## Steps

1. **Check prerequisites** — verify `cargo`, `cargo-tauri` (v2), and `node` are installed:
   ```sh
   cargo --version
   cargo tauri --version   # should be 2.x
   node --version
   ```
   If `cargo tauri` is missing: `cargo install tauri-cli --version "^2"`

2. **Choose frontend** — ask the user which frontend template to use:
   - `vanilla-ts` (TypeScript + HTML, minimal)
   - `react-ts` (React + TypeScript + Vite)
   - `vue-ts` (Vue 3 + TypeScript + Vite)
   - `svelte-ts` (Svelte + TypeScript + Vite)

3. **Scaffold with Tauri CLI**:
   ```sh
   npm create tauri-app@latest . -- --template <chosen-template> --manager npm
   npm install
   ```

4. **Update `src-tauri/tauri.conf.json`**:
   - Set `identifier` to `com.example.<appname>` (required, no spaces)
   - Set `productName` and `version`

5. **Add a sample Tauri command** in `src-tauri/src/lib.rs`:
   ```rust
   #[tauri::command]
   fn greet(name: &str) -> String {
       format!("Hello, {}! From Rust.", name)
   }

   pub fn run() {
       tauri::Builder::default()
           .invoke_handler(tauri::generate_handler![greet])
           .run(tauri::generate_context!())
           .expect("error while running tauri application");
   }
   ```

6. **Wire the command in the frontend** (`src/main.ts` or equivalent):
   ```ts
   import { invoke } from "@tauri-apps/api/core";
   const msg = await invoke<string>("greet", { name: "World" });
   ```

7. **Start dev mode**:
   ```sh
   cargo tauri dev
   ```

## Permissions reminder

If any plugin is used (fs, http, shell, dialog, etc.), declare it in `src-tauri/capabilities/default.json`. Example for filesystem read:
```json
{
  "identifier": "default",
  "description": "Default capability",
  "windows": ["main"],
  "permissions": ["fs:read-all"]
}
```
