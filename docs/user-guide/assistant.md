# AI Assistant

![The AI Assistant pane: asked for the five most populous African countries, it read the Countries table, selected them and zoomed to them](https://assets.rustgis.app/images/assistant.webp)

**Analyze › AI Assistant** (also View › Windows, or Command Search) opens a chat pane. Ask a
question about your data or say what to do with the map, and the assistant answers by running
RustGIS commands and geoprocessing tools for you:

- "Which 5 countries have the largest GDP?" reads the Countries table and answers.
- "Label the cities with their names, and zoom to Tokyo."
- "Buffer the cities by 200 km and colour the result by population."
- "Switch to a dark basemap and hide the rivers."

Each step it takes shows in the conversation (click one to see its parameters and result), and
each change is one undo step, so Ctrl+Z takes back anything you don't want. It never says it
changed the map unless a command succeeded. Commands that would read or write files, connect to
an address on the internet, run a plugin, script or task, or replace the project wait for you:
**Allow** or **Decline**. File and network access can be allowed for the rest of the
conversation, for that exact command and file or address only. Enter sends, Shift+Enter
starts a new line, Up and Down recall earlier messages, and **Stop** ends a run.

From the command line, `rustgis-cli ask` sends one request (see
[Command Line](../cli.md#asking-the-ai-assistant)).

## Choosing a model

Click the model name or the gear to open the settings. The assistant works with:

| Provider | Default model | Key |
| --- | --- | --- |
| Anthropic | `claude-opus-5-5` | [console.anthropic.com](https://console.anthropic.com/settings/keys), or `ANTHROPIC_API_KEY` |
| OpenAI | `gpt-5.5` | [platform.openai.com](https://platform.openai.com/api-keys), or `OPENAI_API_KEY` |
| ChatGPT (sign in) | the first model in your account’s current catalog | none: **Continue with ChatGPT** to use your ChatGPT plan (desktop app and command line) |
| Google Gemini | `gemini-3.8-flash` | [aistudio.google.com](https://aistudio.google.com/apikey), or `GEMINI_API_KEY` / `GOOGLE_API_KEY` |
| OpenRouter | `openrouter/auto` | [openrouter.ai](https://openrouter.ai/keys), or `OPENROUTER_API_KEY` |
| Ollama (local) | `qwen3` | none; the server from `OLLAMA_BASE_URL` (default `http://localhost:11434`) |
| OpenAI-compatible | — | LM Studio, vLLM, llama.cpp…: the address from `OPENAI_COMPATIBLE_BASE_URL`, an optional `OPENAI_COMPATIBLE_API_KEY` |

The ↻ button lists the provider's models. The provider and model you last chose in the settings
are used at start. Until you choose one, `RUSTGIS_ASSISTANT_PROVIDER` and
`RUSTGIS_ASSISTANT_MODEL` pick them, and otherwise the desktop uses the first provider with a key
in the environment. Saved settings win over the environment. Models that call tools well give the best results; small local models may stop early or
skip steps.

### Signing in with ChatGPT

With a ChatGPT plan you can use the assistant without an API key, through OpenAI's
[Sign in with ChatGPT for open-source apps](https://developers.openai.com/siwc/token-sharing-open-source).
Choose **ChatGPT (sign in)** and click **Continue with ChatGPT**: your browser opens OpenAI's
sign-in page, where you approve RustGIS using your plan (the first time, OpenAI registers RustGIS
as an app of your account; you can rename it there). Afterwards the settings show the account's
email, **Using ChatGPT plan**, and a **Weekly usage** link to
[your ChatGPT usage settings](https://chatgpt.com/settings/usage?tab=overview), where you can see and limit
what RustGIS uses or disconnect it. **Sign out** clears local tokens and attempts to revoke the
remote session. If OpenAI does not confirm revocation, RustGIS says so; disconnect RustGIS in
ChatGPT settings to ensure that remote access ends. The registration remains for signing in again.
The account picker retains separate registrations, even when their emails match. **Use another
account** adds a registration; choose an existing one to reuse its issued client ID. From the command line:

```sh
rustgis-cli ask --login                 # continue with ChatGPT in the browser
rustgis-cli ask --login --another-account  # or register RustGIS for another account
rustgis-cli ask --sample --provider chatgpt "Zoom to Tokyo"
rustgis-cli ask --accounts              # list saved registrations
rustgis-cli ask --account oaiapp_...      # select one by its issued client id
rustgis-cli ask --login --account oaiapp_... # reauthorize that registration
rustgis-cli ask --logout
```

The model catalog loads after sign-in, on startup when signed in, and when switching to this
provider or another account. The first model is chosen when the current one is not offered.
The ↻ button reloads the catalog. The CLI uses the catalog's first model unless `--model` or
`RUSTGIS_ASSISTANT_MODEL` is set. While waiting for sign-in, **Open browser again** and
**Copy sign-in link** let you recover a closed tab or failed browser launch. Requests go to the OpenAI API (`https://api.openai.com/v1/responses`) and use
your plan's usage; when a limit is reached, the assistant says so and points to the usage settings.
Signing in needs the desktop app or the command line (it runs a short-lived server on
`127.0.0.1`, port 1455 when it's free, for the browser to return to); the web app doesn't offer it.

Live testing of this integration was limited to a **ChatGPT Pro account on Windows 11**.
Plus, Team/Business, Enterprise and Edu accounts, macOS and Linux have not been live-verified.
Plus has a five-hour usage limit shared across apps using the plan. Team/Business, Enterprise
and Edu workspace policy may block plan usage. Eligibility and model catalogs depend on the
selected account and workspace.

Weekly usage opens ChatGPT's settings page. RustGIS does not show a live percentage: there is
no documented usage endpoint, and the tested live responses supplied no rate-limit headers.

## Privacy

Requests go from RustGIS straight to the provider you chose, with your key. They carry your
messages, the map's layer names, fields and extent, and any rows the assistant reads to answer
you; nothing goes anywhere else. A key typed in the settings is used for the session only unless
you tick **Remember the key on this computer**, which saves it unencrypted in the app's
preferences (in the browser's storage on the web); keys from the environment are never saved.
A ChatGPT sign-in is kept in `chatgpt-auth.json` in the app's settings folder (owner-only on
macOS and Linux; on Windows, protected like the rest of your user profile), never in the
preferences or a project file, and its tokens are never logged; RustGIS renews them as they expire.
The ChatGPT provider's address is fixed, so only the OpenAI API receives them. Renewals are
serialized across RustGIS processes (a refresh token can be used only once).
Plain HTTP is only used for servers on your computer or local network.

In the web app, the browser calls the provider directly. Anthropic, OpenAI, Gemini and
OpenRouter accept that; a local Ollama server must allow the page's origin
(`OLLAMA_ORIGINS=https://web.rustgis.app ollama serve`).
