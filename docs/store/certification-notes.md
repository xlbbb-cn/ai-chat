# Notes for certification — Rabbit Chat Studio

Paste the block below into Partner Center → Submission → **Notes for certification**.
The real test key is **not** stored in this repository — replace `<TEST_API_KEY>` with the value from
the private channel before pasting.

---

## About the app

Rabbit Chat Studio is a desktop AI assistant front-end for OpenAI-compatible chat endpoints. The user
supplies the endpoint, API key and model; the app keeps them locally in
`%APPDATA%\com.leonard.aichat\config.json` and streams answers from that endpoint. There is no account
system, no telemetry, no in-app purchase and no license server.

## How to test (about two minutes)

1. Start the app — it opens on the chat screen.
2. Click the gear icon at the bottom of the left sidebar, then the section **“API & models”**.
3. Enter the test credentials:
   - **Base URL** `https://api.deepseek.com/v1`
   - **API Key** `<TEST_API_KEY>`
   - **Model** `deepseek-flash` (or click **“Fetch Models From API”** and pick `deepseek-flash`)
4. Close settings, type `Hello, reply in one short sentence.` and press Enter. A streamed answer must
   appear, with the model name shown above it.
5. Optional — tool use: send `Create hello.txt in the workspace containing "hi".` A dialog titled
   “Risk Level … — confirm execution” appears first; approve it, the file is created and the assistant
   confirms the result.

The key is a normal paid DeepSeek API key with a small quota, created for this review only. It needs no
VPN, no sign-up and no payment from the reviewer. If it runs out before the review finishes, please
write to us through <https://github.com/xlbbb-cn/ai-chat/issues> and we will supply another one.

## Additional information

- **No sign-in.** All state is local. Uninstalling removes the app; the user’s own files stay in the
  workspace folder they chose.
- **Command execution is user-gated.** Running commands and editing files is the documented purpose of
  the product (see the store description). Every command is scored by a risk engine: safe / read-only
  operations may run directly, everything else shows a confirmation dialog with the risk level and
  waits for the user’s explicit approval. An opt-in setting can auto-approve the highest-risk level;
  it is off by default.
- **Network endpoints contacted**
  1. the chat endpoint the user configures (here `api.deepseek.com`);
  2. `api.github.com`, an update check on startup — toggle in Settings → About → “Check for updates on
     startup”;
  3. optionally a public model-metadata catalogue when the user refreshes the model list.

  Nothing else: no analytics, no advertising SDKs, no crash reporting.
- **runFullTrust** (restricted capability): the assistant reads and writes files in the folder the user
  picks and launches commands the user approves, so a full-trust packaged desktop app is required. No
  other restricted capability is declared.
- **Live generative AI disclosure.** The app sends the user’s prompts to the third-party model endpoint
  the user chooses and renders the returned text. It does not train models, and the developer receives
  neither prompts nor responses. Users can report inappropriate output from the title bar “⋯” menu →
  “Report an issue…”, which opens our public issue tracker (also linked in the store listing).
- **Requirements:** Windows 10 2004 or newer, x64; WebView2 Evergreen runtime (present on Windows 11 and
  on Windows 10 through Edge).
