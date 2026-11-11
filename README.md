# 🎛 Musik Faktory

A kid-friendly **home music studio** on top of the [ElevenLabs Music API](https://elevenlabs.io/docs/overview/capabilities/music),
written in Rust (Axum server + Dioxus web UI), with **parent-controlled spending limits** and **versioned takes**.

- 🎚 **Studio look**: dark console, LCD read-outs, LED budget meter, arrangement timeline with clips, transport deck, big red **● REC** button.
- 💰 **Spending control (parent)**: weekly and/or monthly limit, max track length, a pause switch, price/credit settings, spending log, “give extra budget”.
- ⏱ **No surprise lengths**: the track length is *always* sent explicitly (`music_length_ms` or an exact composition plan). ElevenLabs’ “auto length” is never used.
- 🌳 **Versions (“takes”)**: `v0` → `v1`; go back to `v0` and change something else → `v1.a`; from `v1.a` → `v2.a`, and so on. Every take keeps the exact settings it was made with.
- 🔒🎯 **Edit parts of a take**: in a new take you can *keep* a section exactly as it was (inpainting), make it *new but similar* (audio conditioning), or *brand new*.
- 🤖 **Producer bot**: writes or changes an arrangement from a sentence (the ElevenLabs plan endpoint is free).
- 🧪 **Mock mode**: try everything without an API key; songs are synthesized beeps and cost nothing.

---

## How it fits together

```
 Chromebook (Chrome / installed app)                 Server (one Rust binary)                     ElevenLabs
┌─────────────────────────────────┐   HTTP/JSON   ┌───────────────────────────────────┐  HTTPS  ┌────────────┐
│ Dioxus UI (WebAssembly)         │ ────────────▶ │ budget checks + reservations      │ ──────▶ │ /v1/music  │
│ studio, composer, takes, deck   │ ◀──────────── │ version tree, JSON DB, audio files│ ◀────── │ /plan, ... │
│ parents' control room           │   audio       │ API key (never sent to browser)   │         └────────────┘
└─────────────────────────────────┘               └───────────────────────────────────┘
```

The API key and all the limits live **on the server**, and every limit is enforced there (the UI only mirrors
it). The browser never sees the key. The server is a single self-contained binary (the UI is embedded).

| Crate | What it does |
|---|---|
| `crates/shared` | Types and rules used by both sides: generation parameters, composition-plan model, validation, cost estimate |
| `crates/server` | `musik-faktory` binary: Axum API, ElevenLabs client (+ mock), budget windows, version labels, parent auth, embedded UI |
| `crates/ui` | Dioxus 0.7 web app (compiled to WebAssembly) |

### Spending control in detail

* Before a song starts, the server estimates its cost (`length × price per minute`), checks it against **every active
  limit** and immediately **reserves** it in the spending log (so even a crash or restart can't “forget” it).
  Only one song can record at a time.
* After the song is made, the server reads the ElevenLabs **credit counter** before/after (if the key may read it) and
  charges what was really used, converted with your price per credit. If that number looks unrelated (e.g. something
  else used the account at the same time), it charges the estimate instead.
* Requests ElevenLabs **refuses** (e.g. a prompt naming a real band) are free and the reservation is released.
  Failures where ElevenLabs **may already have billed** (timeouts, dropped connections, server errors, a broken
  answer, the app being closed mid-song) are charged with the measured credits, or with the estimate when nothing can be
  measured, and marked in the log. You can give the money back with “🎁 Give extra budget” if it wasn’t billed.
* The spending log shows every song with estimate, measured credits and charge.
* Weeks start Monday (or Sunday), months are calendar months, both in the server’s local time.
* **Parent sessions** live only in memory: leaving the 🔒 Parents page locks it again, sessions end after 10 idle
  minutes (30 minutes at most), and wrong passwords lock the login for a minute after 5 tries.
* **Hard safety net:** create the ElevenLabs API key with a **credit quota** and restricted to the **Music** endpoints
  (ElevenLabs dashboard → API keys). Even if the key were ever copied off the device, ElevenLabs itself would stop at that quota.
  Allowing “User → read” lets the app measure exact costs; without it, costs are estimated.

Defaults: $0.15/min (ElevenLabs API list price), ~900 credits/min, weekly limit $1.50, monthly $5.00, max length 60 s. Change them in the control room.

---

## Build

Requirements: Rust (stable) and the Dioxus CLI **0.7.9** (the build script adds the needed Rust targets itself).

```bash
cargo install dioxus-cli --version 0.7.9 --locked
```

```bash
scripts/build.sh
```

Results in `dist/`:

| File | What it is |
|---|---|
| `musik-faktory_0.1.0_amd64.deb` | **the installer** for the Chromebook (or any Debian/Ubuntu) |
| `musik-faktory` | the program itself: one self-contained, statically linked file (~9 MB, UI included) |
| `install-linux.sh` | alternative installer that needs no admin rights |

The server is linked statically (musl), so it runs on any Linux with the same CPU type, whatever its glibc version
(a normal build on a recent PC would *not* start on the Chromebook’s Debian 12).

Try it immediately, no key needed:

```bash
dist/musik-faktory serve --mock
```

then open <http://localhost:8787>.

### For an ARM Chromebook

Most Chromebooks are x86_64; many cheaper ones are ARM (`uname -m` in the Linux terminal says `aarch64`). Cross-build
with [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) (the UI is WebAssembly and works everywhere).
This produces `musik-faktory_0.1.0_arm64.deb`:

```bash
cargo install cargo-zigbuild && pip install ziglang
```

```bash
scripts/build.sh aarch64-unknown-linux-musl
```

---

## Install on the Chromebook

### Option A: everything on the Chromebook (simplest)

1. Turn on Linux: *Settings → About ChromeOS → Developers → Linux development environment*.
   (On a Family Link account, a parent may need to allow it.)
2. Copy `musik-faktory_0.1.0_amd64.deb` into **Linux files** with the Files app, then **double-click it**
   (or right-click → *Install with Linux*) and confirm.
3. **Musik Faktory** appears in the launcher (in the *Linux apps* folder; drag it to the shelf if you like).
   Clicking it starts the studio server and opens the app in Chrome. Optionally use Chrome’s *Install app*
   button (`http://localhost:8787` is a secure origin, so it can be installed as a PWA).
4. Open **🔒 Parents**, choose the parent password, the limits, and add the API key (see below).

To uninstall: right-click the launcher icon → *Uninstall*. Songs and settings stay in
`~/.local/share/musik-faktory/` (delete that folder to remove them too).

No admin rights? `bash dist/install-linux.sh` installs the same into your home folder.

Note: in this mode the API key is stored inside the Chromebook’s Linux container
(`~/.local/share/musik-faktory/db.json`). That’s why the **credit quota on the key** is recommended.

### Option B: server at home, Chromebook only needs Chrome (most tamper-proof)

Run the server on an always-on machine (home PC, Raspberry Pi, NAS). On Debian/Ubuntu, install the same `.deb`, or
just run the binary:

```bash
musik-faktory serve --bind 0.0.0.0 --port 8787
```

On the Chromebook open `http://<that-machine>:8787` (an IP address, the machine’s own host name or `<name>.local` work;
add other names with `--allowed-host musicbox.example,*.ts.net`. Unknown host names are refused to block DNS-rebinding
tricks from web pages) and use *⋮ → Cast, save and share → Create shortcut → Open as window*.
For a real installable app and access outside home, put it behind HTTPS (e.g. `tailscale serve 8787`).
The key never leaves the server. The kid pages have no login, so anyone on your home network can use the
studio, always within the limits. Only parents can change the limits.

---

## The ElevenLabs API key

**Get a key:** <https://elevenlabs.io/app/settings/api-keys> → *Create API key*. Recommended settings:

* **Restrict** it to the **Music** endpoints (and allow *User → read* so the app can measure exact costs).
* Give it a **credit quota**: ElevenLabs itself then stops at that amount, a hard cap on top of the app’s limits.

**Put it into the app**, one of these:

1. **Parents page, first setup:** the field *“ElevenLabs API key (can be added later)”*.
2. **Parents page → Control room:** the *ElevenLabs key* panel (shown at the top, highlighted, while no key is set):
   paste it, press **Save & test**. The app checks it with ElevenLabs and refuses invalid keys.
3. **Terminal** (server stopped), handy for a home server:

   ```bash
   musik-faktory set-api-key
   ```

4. **Environment variable** (takes priority over the stored key): `ELEVENLABS_API_KEY=sk_… musik-faktory serve`.

The key is stored on the server only (`~/.local/share/musik-faktory/db.json`) and is never sent to the browser;
the control room shows it masked (`sk_a…wxyz`).

## Useful commands

| Command | What it does |
|---|---|
| `musik-faktory serve [--mock] [--bind 0.0.0.0] [--port 8787] [--data-dir DIR] [--allowed-host NAME,...]` | run the server |
| `musik-faktory open` | start the server if needed and open the browser (used by the launcher) |
| `musik-faktory reset-password` | forget the parent password (stop the server first) |
| `musik-faktory set-api-key [KEY]` | save and test the ElevenLabs key (stop the server first) |
| `ELEVENLABS_API_KEY=… musik-faktory serve` | use a key from the environment instead of the stored one |

Data (tracks, audio, settings, spending log) lives in `~/.local/share/musik-faktory/`. Back that folder up.

---

## What the child can set

Everything the Music API offers for generation:

| In the studio | ElevenLabs parameter |
|---|---|
| 💬 Describe it: text, length fader, “instrumental only” | `prompt`, `music_length_ms` (always set), `force_instrumental` |
| Experimental: kind of piece, own lyrics | `generation_mode` (track/loop/ambience), `lyrics_text` |
| 🧩 Arrange it: sections as clips (name, lyrics & `{cues}`, length, sounds wanted/avoided, fit with neighbours) | `composition_plan.chunks[]`: `text`, `duration_ms`, `positive_styles`, `negative_styles`, `context_adherence` |
| 🔒 Keep from vX | audio reference chunk `{song_id, range}` (inpainting) |
| 🎯 New, but similar + how similar | `conditioning_ref`, `condition_strength` |
| Seed (🎲 / 📌 / “use v0’s seed”) | `seed` (plans only; the app records a random one so takes can be reproduced) |
| Studio rack: model, audio quality, sound pack + strength, name pronunciation, C2PA signing, remember for editing | `model_id` (v2.5/v2), `output_format`, `finetune_id`, `finetune_strength`, `use_phonetic_names`, `sign_with_c2pa`, `store_for_inpainting` |
| 🤖 Producer bot | `POST /v1/music/plan` with `prompt`, `music_length_ms`, `source_composition_plan` |

Deliberately not exposed: `music_v1` (deprecated, removed by ElevenLabs on 2027-01-31) and its section-plan format,
`respect_sections_durations` (v1 only), `enable_logging=false` (enterprise only), `music_prompt` (deprecated),
`with_timestamps` / `with_waveform_visual`.
Possible extensions: stem separation (`/v1/music/stem-separation`, “give me only the vocals”), uploading a song,
video-to-music, karaoke lyrics via word timestamps.

---

## Development

```bash
cargo test
```

Run the API in mock mode in one terminal:

```bash
cargo run -p musik-faktory -- serve --mock
```

and the UI with hot reload in another (it proxies `/api` to port 8787):

```bash
cd crates/ui && dx serve --platform web
```

---

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| **This design: Axum server + Dioxus web UI (PWA)** | Key and limits server-side; Chromebook only needs Chrome; one binary; same code runs on the Chromebook or a home server | Needs a running server (local service or home machine) |
| Dioxus **fullstack** (server functions) | Even less glue code: call server functions like Rust functions | Tighter coupling to the `dx` toolchain; harder to cross-build/deploy as a plain binary |
| Dioxus **desktop** in Crostini | One native window, no browser | API key and limits live on the kid’s device; webkit2gtk makes ARM builds harder; Linux must be enabled |
| **Tauri 2** (Rust + web UI) | Very mature, can also produce an **Android** app (Chromebooks run Android apps) | Key on the device again unless you keep a backend; Android signing/distribution effort |
| **Leptos + Axum** | Excellent Rust full-stack framework, fine-grained reactivity, SSR | Same architecture as here, just a different UI framework |
| **Slint / egui** (native Rust GUI) | Small, fast, no web tech | Runs only as a Linux app in Crostini; less “web/app-like”; same key-on-device issue |
| SvelteKit/Next.js PWA + serverless (Cloudflare Workers, Vercel) | Works anywhere with HTTPS, no home server | Not Rust; you host a small backend in the cloud |
| Flutter app | Polished Android/ChromeOS app | Not Rust; key handling still needs a backend for real spending control |

Rule of thumb: **wherever the API key lives, the spending limits must be enforced there too.**
Here that’s the server, plus ElevenLabs’ own per-key credit quota as a backstop.

---

## License

[MIT](LICENSE). Eleven Music output is subject to the [ElevenLabs music terms](https://elevenlabs.io/music-terms).
