# Cloud integrations with the user's own API key

Surveyed 2026-10-03. Every claim below comes from the vendor's own
documentation, pricing page or API reference on that day. The URL is under
each section. Where a vendor page did not say something, this document says
"not documented" and does not guess. Prices change often in this market. Treat
every number as "true on the survey date", and read it again before you ship a
cost table.

The question for each service was narrow:

1. Can a user paste **their own** API key into chukcut and use the service?
   No OAuth app that the project must register, and no proxy server that the
   project must run.
2. If yes, what does it give the editor, what does it cost the user, and what
   may the user do with the result?

Offline and local stay the default. Every integration here is an optional
extra. With no key set, the feature is not there and nothing calls out.

## Summary

| Service | Capability | Auth | User key possible? | Request shape | Verdict |
|---|---|---|---|---|---|
| ElevenLabs | TTS, voice library, voice changer, dubbing, SFX, music, STT | `xi-api-key` header | **yes** | sync (TTS, SFX, music, voice changer); async job (dubbing) | **integrate (1st wave)** |
| OpenAI | TTS, transcription, images | Bearer key | **yes** | sync | **integrate (1st wave)**, as "OpenAI-compatible" |
| Generic OpenAI-compatible TTS / STT (Kokoro-FastAPI, Groq, local servers) | TTS, transcription | Bearer key + base URL | **yes** | sync | **integrate (1st wave)** |
| Groq | transcription (Whisper) | Bearer key, OpenAI format | **yes** | sync | **integrate** (cheapest STT, same code as OpenAI) |
| fal.ai | one key, hundreds of models: video gen, upscale, background removal, interpolation, lip-sync, image gen | `Authorization: Key …` | **yes** | async queue + poll / SSE | **integrate (2nd wave)**, the main Generate provider |
| Higgsfield | image and video gen, its own models plus Kling, Seedance | `Authorization: Key id:secret` | **yes** | async + poll | **integrate after fal** (same trait) |
| Replicate | one key, many models | Bearer key | **yes** | async (or `Prefer: wait`) | later; fal covers the same ground |
| Runway | video gen, upscale, image | Bearer + `X-Runway-Version` | **yes** | async task + poll | later |
| Luma | video and image gen | Bearer key | **yes** | async + poll | later |
| Kling (direct) | video gen | Bearer key | **yes** | async + callback/poll | later; reachable through fal and Higgsfield |
| Google Gemini API (Veo, Gemini TTS, image) | video, TTS, image | `x-goog-api-key` | **yes** | Veo async (long-running op); TTS sync | later |
| Pika | video gen | via **fal key** | yes (through fal) | async | comes free with fal |
| Cartesia | TTS | Bearer key + `Cartesia-Version` | **yes** | sync | later |
| Azure Speech | TTS, STT | `Ocp-Apim-Subscription-Key` + region | **yes** | sync | later |
| Google Cloud TTS | TTS | API key (`key=` / `x-goog-api-key`) in practice; docs push service accounts | yes, with care | sync | later; Gemini TTS is simpler |
| PlayHT | TTS | — | **no, service shut down** (Dec 2025) | — | **not possible** |
| Stability AI (Stable Audio) | music / SFX gen | Bearer key | **yes** | sync (2.x) / async (3.0) | **integrate (3rd wave)** with ElevenLabs Music |
| Suno | music gen | — | **no public API** (partner intake only) | — | **not possible** |
| Udio | music gen | — | **no API; downloads disabled** since Oct 2025 | — | **not possible** |
| Deepgram | transcription | `Token` key, own format | **yes** | sync, optional callback | later |
| AssemblyAI | transcription | key header, own format | **yes** | async + poll | later |
| DeepL | caption translation | `DeepL-Auth-Key` | **yes** | sync | **integrate (2nd wave)** |
| Epidemic Sound | licensed music + SFX | partner API key (backend only) or ES Connect OAuth (registered app) | **no** | — | **not possible** under our rules |
| Artlist | licensed music | enterprise contract | **no** | — | **not possible** |
| Soundstripe | licensed music | partner key, backend proxy required | **no** | — | **not possible** |
| Musicbed | licensed music | no public API | **no** | — | **not possible** |
| Uppbeat | free/paid music | no public API found | **no** | — | **not possible** |
| Pixabay Music | free music | Pixabay API has no audio endpoint | **no** | — | **not possible** (images and video: yes) |
| Pexels | stock photo + video | `Authorization` key | **yes** | sync | **integrate (1st wave, cheap)** |
| Pixabay (images, video) | stock photo + video | `key=` param | **yes** | sync | **integrate (1st wave, cheap)** |
| Freesound | SFX | token key (search, previews); OAuth2 for originals with a **user-owned** client | **yes** | sync | integrate with the asset browser |

Three findings matter more than the rest:

- **No licensed music library allows our model.** Epidemic Sound, Artlist,
  Soundstripe and Musicbed all sell to *platforms*, under contract, with the
  key on the platform's server. A user cannot get a personal key. A music
  library in chukcut must come from AI music generation (ElevenLabs Music,
  Stable Audio), from open libraries (the open-assets survey), or from the
  user's own downloads. §3 has the details.
- **fal.ai is the best single key for "Generate".** One key reaches video
  generation (Kling, Veo, Seedance, MiniMax, Pika), and also the editing
  tools a CapCut clone needs: video background removal, video upscale,
  frame interpolation and lip-sync. It has a pricing API, so chukcut can
  show the cost before a job runs. §4.2.
- **ElevenLabs is the best single key for audio.** TTS with character
  timestamps (free word timing for captions on a voiceover), sound effects,
  music, voice changer and dubbing, all with one `xi-api-key`. But its free
  tier forbids commercial use. chukcut must record the plan tier with each
  result and warn at export. §1.1.

Two services left the market during 2025–2026. Do not build for them:
**PlayHT** shut down (API offline July 2025, service ended 31 Dec 2025), and
**OpenAI's Sora video API** is deprecated as of 24 Sep 2026.

---

## The "own key" rule, and why some vendors say "server only"

Most vendors write "never put the API key in client code". Higgsfield,
Soundstripe, Epidemic Sound and DeepL all say this. The warning is about a
company that ships **its own** key inside an app that strangers download.
Anyone can extract that key and spend the company's money.

That is not our case. In chukcut, the user pastes a key that they created in
their own account. The key stays on their own machine. Nobody else gets it.
That is the same trust model as a CLI tool that reads the key from the
user's config. Vendors that sell metered usage to individual developers
(ElevenLabs, OpenAI, fal, Replicate, Higgsfield, Runway, Stability, Luma,
Kling, Deepgram, AssemblyAI, DeepL, Pexels, Pixabay, Freesound) all support
this use.

Partner APIs are different. Epidemic Sound, Soundstripe and Artlist give a key
only to a company with a contract, and the contract makes the company
responsible for the licence of every downloaded track. A user cannot get such a
key, and a key from a partner must not leave the partner's server. These
services are not possible without a contract and a server. That breaks two
of the owner's rules.

---

## 1. Text to speech and voices

### 1.1 ElevenLabs

**Auth.** Header `xi-api-key: <key>`. A key can have endpoint scopes, a
credit quota and an IP allowlist. A user can make a key for chukcut that can
only do TTS and SFX and can spend at most N credits. Recommend this in the
settings help text.

**What it gives the editor.**

| Feature | Endpoint | Shape | Notes |
|---|---|---|---|
| TTS | `POST /v1/text-to-speech/{voice_id}` | sync, audio bytes | formats: MP3, PCM, WAV, Opus, μ-law, A-law; `seed`; `previous_text`/`next_text` for continuity across clips |
| TTS + timing | `POST /v1/text-to-speech/{voice_id}/with-timestamps` | sync, JSON: `audio_base64` + `alignment` (characters, `character_start_times_seconds`, `character_end_times_seconds`) | **word timing for captions comes free with the voiceover** — no transcription pass |
| Voice list | `GET /v2/voices` | paged (`next_page_token`, `has_more`, max 100) | filters for gender, age, language, accent, category; each voice has a `preview_url` for audition in the UI |
| Voice library (community voices) | shared-voices endpoints | paged | **not available via API on the free tier**; voice owners can set a notice period (30 days – 2 years), after which the voice goes away (`disable_at_unix`) |
| Voice changer | `POST /v1/speech-to-speech/{voice_id}` | sync, multipart audio in, audio out | `remove_background_noise` option; good for "change my voiceover to voice X" |
| Sound effects | `POST /v1/sound-generation` | sync | `duration_seconds` 0.5–30, `prompt_influence` 0–1, `loop` (v2 model) for seamless loops |
| Music | `POST /v1/music` | sync | `prompt` or a section-by-section `composition_plan`; length 3 s – 600 s; `force_instrumental`; optional C2PA signing |
| Dubbing | `POST /v1/dubbing` | **async**: returns `dubbing_id` + `expected_duration_sec`; poll `GET /v1/dubbing/{id}` until `status` is `dubbed`; download is a signed URL valid "about an hour" | `watermark` flag; v1 is watermarked |
| Usage | `GET /v1/user/subscription` | sync | `character_count`, `character_limit`, `tier`, reset time — use for the **test-connection button** and the remaining-credit display |

**Models (TTS).** `eleven_v4` (10k chars/request), `eleven_v4_turbo`
(~100 ms), `eleven_v3` (5k chars), `eleven_multilingual_v2` (10k chars, the
API default), `eleven_flash_v2_5` (~75 ms, 40k chars). An editor does not need
low latency. Default to the best-quality model and let the user choose.

**Concurrency** (simultaneous requests, Multilingual v2 / Flash): Free 2/4,
Starter 3/6, Creator 5/10, Pro 10/20, Scale 15/30. The job queue must limit
parallel jobs per provider to this value. The API returns 429 above it.

**Pricing.** Free plan: 10k credits per month, **no commercial licence**.
Starter $6/month is the first plan with a commercial licence (also for
music). Pay-as-you-go API prices on the survey day: TTS $0.04–0.08 per 1k
characters (v4 models were on a time-limited discount), SFX $0.12/min,
music $0.15/min, voice changer $0.12/min, dubbing v1 $0.33/min (watermarked),
v2 $2.20/min, Scribe STT $0.22/h.

**Output use.** Commercial use needs a paid plan. A video with free-tier
audio in it is a claim risk on a monetised channel. chukcut must save the
`tier` from `/v1/user/subscription` with each generated file and warn at export
when a free-tier asset is on the timeline.

**Verdict: integrate, first wave.** TTS with timestamps, SFX and the voice list
first. Music in the third wave. Voice changer and dubbing later, because they
upload the user's media and dubbing is async.

Sources:
<https://elevenlabs.io/docs/api-reference/authentication>,
<https://elevenlabs.io/docs/api-reference/text-to-speech/convert>,
<https://elevenlabs.io/docs/api-reference/text-to-speech/convert-with-timestamps>,
<https://elevenlabs.io/docs/api-reference/voices/search>,
<https://elevenlabs.io/docs/eleven-creative/voices/voice-library.md>,
<https://elevenlabs.io/docs/api-reference/speech-to-speech/convert>,
<https://elevenlabs.io/docs/api-reference/text-to-sound-effects/convert>,
<https://elevenlabs.io/docs/api-reference/music/compose>,
<https://elevenlabs.io/docs/api-reference/dubbing/create>,
<https://elevenlabs.io/docs/api-reference/dubbing/get>,
<https://elevenlabs.io/docs/eleven-api/guides/how-to/dubbing/manage-projects.md>,
<https://elevenlabs.io/docs/api-reference/user/subscription/get>,
<https://elevenlabs.io/docs/overview/models.md>,
<https://elevenlabs.io/pricing>, <https://elevenlabs.io/pricing/api>

### 1.2 OpenAI TTS

**Auth.** `Authorization: Bearer <key>`.

**Endpoint.** `POST /v1/audio/speech` with `model`, `input`, `voice`, and
optional `instructions` (accent, tone, emotion, speed in plain words). Sync.
Formats: MP3, Opus, AAC, FLAC, WAV, PCM.

**Models and voices.** `gpt-4o-mini-tts` (13 voices: alloy, ash, ballad,
coral, echo, fable, nova, onyx, sage, shimmer, verse, marin, cedar; OpenAI
recommends marin or cedar), and the older `tts-1` / `tts-1-hd` (9 voices).
50+ languages, tuned for English.

**Output use.** OpenAI's usage policy requires "a clear disclosure to end users
that the TTS voice they are hearing is AI-generated". That is a rule on the
user's account. A breach can get the account closed. Show it once in the
provider's help text.

**No timestamps.** Unlike ElevenLabs, the response has no timing. Captions for
an OpenAI voiceover need a transcription pass (cheap; §5).

**Verdict: integrate, first wave**, as the reference implementation of the
generic "OpenAI-compatible TTS" provider (§1.6).

Sources: <https://developers.openai.com/api/docs/guides/text-to-speech>,
<https://developers.openai.com/api/docs/pricing>

### 1.3 Cartesia

**Auth.** `Authorization: Bearer sk_car_…` plus a required
`Cartesia-Version` header (the reference showed `2026-08-14`).

**Endpoint.** `POST https://api.cartesia.ai/tts/bytes`, sync, audio bytes.
Body: `model_id` (`sonic-3.6`, `sonic-3.5`, `sonic-3`, `sonic-latest`),
`transcript`, `voice` (id), `output_format` (WAV, MP3, RAW), optional
`language`, `generation_config` (volume 0.5–2, speed 0.6–1.5, emotion).

**Pricing.** Free: 20k credits/month. Pro $5/month (100k credits, instant
voice cloning, **commercial licence from Pro up**). TTS costs about 750–800
credits per minute of audio.

**Verdict: later.** Good quality and a cheap entry plan, but a second premium
TTS adds little when ElevenLabs is there. It is a small provider to add once
the `Tts` trait exists.

Sources: <https://docs.cartesia.ai/api-reference/tts/bytes>,
<https://cartesia.ai/pricing>

### 1.4 Google: Gemini API TTS and Cloud Text-to-Speech

**Gemini API TTS** (AI Studio key, header `x-goog-api-key`). The current
models are the `gemini-*-flash-tts` family. Unary requests return 24 kHz mono
16-bit PCM (WAV). Up to two speakers in one request, 30 prebuilt voices plus
an extended library. **There is a free tier**, but on the free tier Google
uses the content to improve its products. The paid tier does not.

**Cloud Text-to-Speech** (`texttospeech.googleapis.com`). The authentication
page only describes ADC and service accounts. In practice the REST API also
accepts an API key (`?key=` or `x-goog-api-key`), and third-party apps use
this (for example Home Assistant). The user must create a GCP project,
enable billing, enable the API and then create a key. That is too many steps
for an editor user.

**Verdict: later.** If we add Google, add the Gemini API (one key from AI
Studio, a free tier, and the same key gives Veo and image models; §4.7).
Do not add Cloud TTS.

Sources: <https://ai.google.dev/gemini-api/docs/speech-generation>,
<https://ai.google.dev/gemini-api/docs/pricing>,
<https://docs.cloud.google.com/text-to-speech/docs/authentication>,
<https://docs.cloud.google.com/docs/authentication/api-keys-use>

### 1.5 Azure Speech

**Auth.** `Ocp-Apim-Subscription-Key: <resource key>` against a **regional**
endpoint (`https://<region>.tts.speech.microsoft.com/cognitiveservices/v1`),
so the settings need *key + region*. A 10-minute bearer token from
`issueToken` is optional.

**Request.** `POST`, body is SSML, output format in the
`X-Microsoft-OutputFormat` header (up to 48 kHz). **`User-Agent` is
required.** Send `chukcut/<version>` and nothing more. Audio over 10 minutes is
cut off. `GET …/voices/list` returns all voices with styles and a
`WordsPerMinute` value. That value gives a duration estimate before
synthesis.

**Pricing.** Free tier F0: 0.5 M neural characters/month and 5 audio
hours/month STT. Paid prices depend on region.

**Verdict: later.** Many voices and styles, and a generous free tier. But the
Azure portal setup (resource, region, key) is a lot to ask.

Source: <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech>,
<https://azure.microsoft.com/en-us/pricing/details/speech/>

### 1.6 PlayHT

**Not possible.** Meta acquired PlayAI in July 2025. The API went offline on
26 July 2025, and the service ended on 31 December 2025. The `play.ht` domain
does not resolve any more (DNS lookup failed on the survey day).

Sources: <https://anyspeech.io/playht-alternatives>,
<https://cognitivefuture.ai/elevenlabs-vs-playht-alternatives/> (secondary
sources; the vendor's own site is gone)

### 1.7 Generic OpenAI-compatible TTS

Many servers copy OpenAI's `POST /v1/audio/speech` shape: `{model, input,
voice, response_format, speed}` in, audio bytes out. Examples:
**Kokoro-FastAPI** (Apache-2.0 model and code, runs locally, also has a
`/dev/captioned_speech` endpoint with word timestamps) and **LocalAI**.

One provider type, "OpenAI-compatible TTS", with three fields (base URL, key,
model) and a free-text voice field covers OpenAI, any hosted clone, and a
local server on `http://127.0.0.1:8880`. The last one is the bridge between
"cloud extra" and "offline default". A user who runs Kokoro locally gets good
offline TTS through the same UI.

**Verdict: integrate, first wave.** It is the same code as §1.2.

Source: <https://github.com/remsky/Kokoro-FastAPI>

---

## 2. Sound effects and music generation

### 2.1 ElevenLabs Sound Effects and Music

See §1.1. SFX: sync, 0.5–30 s, loopable, $0.12/min. Music: sync, 3 s – 10 min,
optional instrumental, $0.15/min; commercial use from Starter up. Both cover
the CapCut "sound effects" and "generate music" buttons with one key.

### 2.2 Stability AI — Stable Audio

**Auth.** `Authorization: Bearer sk-…`, `Accept: audio/*`.

**Endpoints.**

| Model | Path | Shape | Cost |
|---|---|---|---|
| Stable Audio 2 / 2.5 | `POST /v2beta/audio/stable-audio-2/{text-to-audio,audio-to-audio,inpaint}` | sync, audio bytes | 20 credits ($0.20) per generation, up to 3 min, 44.1 kHz stereo |
| Stable Audio 3.0 | `POST /v2beta/audio/stable-audio/{text-to-audio,audio-to-audio,inpaint}` | **async**: HTTP 202 + id, poll `GET /v2beta/audio/results/{id}` | 26 credits ($0.26), up to 6 min |

1 credit = $0.01. New accounts get 25 free credits. Rate limit: 150 requests
per 10 s, then a 60 s timeout.

**Output use.** Stability's licence page says you own the outputs of its
models and can use them commercially. The free Community Licence covers
organisations under $1 M annual revenue. The page talks about self-hosted
models and does not say clearly how this applies to API output. Record
"commercial: yes (Stability terms)", with the link.

Audio inpaint and audio-to-audio are useful in an editor: "extend this music
bed by 20 s" or "make a variation of this track".

**Verdict: integrate, third wave**, next to ElevenLabs Music. Flat cost per
generation makes the cost estimate exact.

Sources: <https://platform.stability.ai/docs/api-reference>,
<https://platform.stability.ai/pricing>, <https://stability.ai/license>

### 2.3 Suno

**Not possible.** No public, self-serve API. On 1 July 2026 Suno opened an
intake form for "a curated group of partners" for a future developer API. No
timeline. Third-party "Suno APIs" drive the web app with a stolen session.
They break often, and they are against Suno's terms, so the user's account
can get banned. Do not integrate them.

Sources: <https://www.digitalmusicnews.com/2026/07/03/suno-is-opening-an-api-partner-program/>

### 2.4 Udio

**Not possible.** No public API. After the settlement with Universal Music
Group (29 Oct 2025), Udio disabled downloads on 30 Oct 2025 and became a
"walled garden". Users can stream their songs inside Udio but cannot take them
out. Even a manual import into chukcut is impossible.

Sources: <https://www.digitalmusicnews.com/2025/10/31/udio-downloads-disabled-umg-deal/>,
<https://routenote.com/blog/udio-stops-user-downloads-after-umg-deal-heres-why/>

---

## 3. Licensed music libraries

The owner wants in-app browse and download with the user's own subscription.
**No library in this list allows that.**

### 3.1 Epidemic Sound

Epidemic has a real, good API: 55k tracks, 250k+ SFX, semantic search,
"find similar", "search by video" (Soundmatch), highlights for short-form,
and usage reporting. But all the auth methods fail our rules:

| Method | What it is | Why it fails |
|---|---|---|
| API key (`epidemic_live_…`) | partner key from the Developer Portal | "The API key stays on your server and all requests are proxied through your own backend." Portal access is not self-serve: "please reach out to us to discuss a partnership". Needs a contract **and** a server. |
| Partner token + user token (legacy) | backend exchanges partner credentials for tokens | same: partner credentials, backend only |
| **ES Connect** (OAuth 2.0 + PKCE) | the user logs in with their **own Epidemic subscription** and gets full library access at their tier | the app must be **registered in the Developer Portal**: "You enable Connect and register your redirect URIs yourself, in your app's settings… Epidemic Sound does not add redirect URIs or issue client IDs on request." Portal access needs a partnership. |

ES Connect is technically the right flow for chukcut. It is PKCE, so there
is no client secret, and a public client id could ship in a GPL binary. The
user's own subscription would cover the licence of every track. But someone
must register the app with Epidemic first. The project would have to do it,
and the owner rules that out. The developer site also mentions a free plan
"for prototyping only — you cannot go live on this plan", with 50 downloads.
That plan does not allow a released app either.

**Verdict: not possible under the current rules.** If the owner ever accepts
one partner registration, ES Connect is the only music-library integration
worth applying for. It needs no server, the user pays Epidemic directly, and
the licence is the user's. Record this as the one exception to consider.

Sources: <https://developers.epidemicsound.com/>,
<https://developers.epidemicsound.com/docs/auth/>,
<https://developers.epidemicsound.com/docs/getting-started/>,
<https://developers.epidemicsound.com/docs/faq/>

### 3.2 Artlist

The Artlist Music API is an **Enterprise** product. Access needs "an Artlist
Enterprise API" contract and an account manager
(`enterprise-api-support@artlist.io`). Artlist does not offer personal keys
for subscribers. **Not possible.**

Sources: <https://developer.artlist.io/welcome>,
<https://artlist.io/blog/your-fast-track-to-better-app-audio-with-the-artlist-music-api/>

### 3.3 Soundstripe

The API exists (`api.soundstripe.com`, JSON:API, token auth, 25 req/s), but
"Client-side applications are prohibited from communicating with
Soundstripe's API due to the privileged nature of API keys". You must proxy
through your own backend. Keys come through a partner programme. **Not
possible.**

Sources: <https://docs.soundstripe.com/docs/integrating-soundstripes-content-into-your-application>,
<https://docs.soundstripe.com/reference/rate-limits.md>

### 3.4 Musicbed, Uppbeat

No public developer API found for either (searched on the survey day; no
developer documentation exists on either site). **Not possible.**

### 3.5 Pixabay Music

The Pixabay API covers **images and videos only** (`/api/` and
`/api/videos/`). There is no audio endpoint. **Not possible** for music.
Note for the open-assets survey: the Pixabay Content License allows
commercial use without attribution, but Pixabay music is known to trigger
Content ID claims, which can block or redirect monetisation.

Sources: <https://pixabay.com/api/docs/>,
<https://pixabay.com/service/license-summary/>

### 3.6 What chukcut can do instead

- **Generated music** (ElevenLabs Music, Stable Audio): the user's key, the
  user's licence, no library browsing but "describe the track you want".
- **Open libraries** (Freesound, CC0 / CC-BY music): see the open-assets
  survey.
- **Import a folder**: users who pay Epidemic or Artlist download tracks in
  the vendor's own app. chukcut can watch a "music library" folder, index it
  (duration, BPM, waveform), and show it in the audio tab like a built-in
  library. No vendor deal needed. This gives most of the value.

---

## 4. Image and video generation and effects

Common facts for this group. A desktop app cannot receive webhooks, so every
provider here is used by **polling** (or fal's SSE stream). Output URLs
**expire**: Replicate after 1 hour, Higgsfield after 7 days, Veo after 2
days, and Runway after an undocumented time. chukcut must download the result
when the job ends, not when the user clicks on it.

### 4.1 Higgsfield

**The plugin the owner remembers.** Higgsfield ships official plugins for
**Adobe Premiere Pro and After Effects** (one installer), **DaVinci Resolve**,
Photoshop, Figma/FigJam, and a Blender bridge. Features in the
Premiere/Resolve plugins: generate video, generate image, reframe, remove
background, upscale (to 4K), draw-to-edit/draw-to-video, edit video, colour
match, import to timeline. "All integrations use your plan balance at the
same rates as the web platform." So the plugins spend the user's own
Higgsfield credits. The plugin pages do not say how the plugins log in.

**Public API: yes, with keys.** Credentials come from Higgsfield Cloud
(`console.higgsfield.ai` / `cloud.higgsfield.ai`) as a key id plus a secret.
Header: `Authorization: Key <KEY_ID>:<KEY_SECRET>` (legacy `hf-api-key` and
`hf-secret` headers still work). Base URL `https://api.higgsfield.ai`.

**Shape.** Async. Submit: `POST https://api.higgsfield.ai/<model path>`
(example: `/higgsfield-ai/soul/v2/standard` with `{"prompt": …}`). The answer
has `request_id`, `status_url` and `cancel_url`. States: `queued` →
`in_progress` → `completed` | `failed` | `nsfw` | `canceled`. Cancel works
only while queued. Output: `images[]`, `video.url`, or `audio`/`audios[]`.
Use an `Idempotency-Key` header for safe retries. Webhooks exist (not usable
on a desktop).

**Billing.** Account credits per successful request. `failed` and `nsfw` are
not charged, and a cancel before processing is refunded. The docs mention an
**estimate endpoint** for the cost before submitting (path not found in the
pages read; find it in the API reference before implementing). The docs do
not say if API credits are the same pool as a web subscription.

**Rate limits.** Concurrency per account and model (example error: "Maximum
number of concurrent requests (4) has been reached", HTTP 400). There are
no rate-limit headers and no `Retry-After`. The job queue must enforce a
per-provider concurrency setting.

**Output use.** The API docs do not say anything about commercial use. Read
Higgsfield's terms of service before you show a "commercial: yes" badge.

**Verdict: integrate after fal.** It is the same `Generate` trait and the same
poll loop. The owner already uses Higgsfield, so his credits and his trained
characters (Soul) become usable inside the editor. Reframe, background removal
and upscale from the plugin feature list are also in fal's catalogue (§4.2).

Sources: <https://docs.higgsfield.ai/docs/authentication.md>,
<https://docs.higgsfield.ai/docs/quickstart.md>,
<https://docs.higgsfield.ai/docs/concepts/requests.md>,
<https://docs.higgsfield.ai/docs/concepts/billing-and-retention.md>,
<https://docs.higgsfield.ai/docs/concepts/rate-limits.md>,
<https://higgsfield.ai/plugins/premiere-pro>,
<https://higgsfield.ai/creator-hub/help-center/integrations/external-integrations-higgsfield>,
<https://nofilmschool.com/higgsfield-adobe-ai-plugin>

### 4.2 fal.ai — one key, many models

**Auth.** `Authorization: Key <FAL_KEY>`.

**Shape.** One queue API for every model:

| Step | Request |
|---|---|
| submit | `POST https://queue.fal.run/{model-id}` with the model's JSON input; answer has `request_id`, `status_url`, `response_url`, `cancel_url`, `queue_position` |
| status | `GET …/requests/{id}/status?logs=1` → `IN_QUEUE` / `IN_PROGRESS` / `COMPLETED` |
| status stream | `GET …/requests/{id}/status/stream` — **Server-Sent Events**, so progress and logs arrive with no polling |
| result | `GET …/requests/{id}` |
| cancel | `PUT …/requests/{id}/cancel` |
| price | `GET https://api.fal.ai/v1/models/pricing?endpoint_id=<id>` → unit price, billing unit, currency |

fal re-queues a failed runner up to 10 times. You pay only for successful
outputs, not for queue time or server errors. File inputs (a clip for
background removal) go up as URLs. fal has a storage upload for this. The
queue doc did not cover it, so read the storage docs before implementing.

**Model catalogue relevant to an editor** (endpoint ids seen on fal.ai on the
survey day):

| Editor feature | fal endpoint ids |
|---|---|
| Text/image to video | `fal-ai/kling-video/v3/pro/image-to-video`, `fal-ai/kling-video/v3/turbo/pro/{text,image}-to-video`, `fal-ai/veo3.1`, `bytedance/seedance-2.5/image-to-video`, `minimax/h3-max/{text,image}-to-video`, `blackforestlabs/flux-3/image-to-video`, `fal-ai/pika/v2.2/*` (Pika's official API is on fal) |
| **Video background removal** (CapCut "remove background") | `bria/video/background-removal` (+ `/v3`, `/realtime`, `/green-screen-despill`), `veed/video-background-removal` (+ `/fast`, `/green-screen`), `pixelcut/video-background-removal`, `fal-ai/birefnet/v2/video`, `fal-ai/ben/v2/video` |
| **Video upscale** | `topaz/upscale/video/{precision,creative,generative}`, `fal-ai/seedvr/upscale/video`, `fal-ai/flashvsr/upscale/video`, `fal-ai/bytedance-upscaler/upscale/video`, `fal-ai/video-upscaler`, `clarityai/crystal-video-upscaler` |
| **Frame interpolation** (smooth slow-mo) | `fal-ai/rife/video`, `fal-ai/film/video`, `fal-ai/amt-interpolation`, `topaz/interpolate/video` |
| **Lip-sync** (dub + mouth) | `fal-ai/sync-lipsync/v3`, `fal-ai/sync-lipsync/v2/pro`, `fal-ai/latentsync`, `veed/lipsync/v2`, `fal-ai/kling-video/lipsync/audio-to-video`, `fal-ai/heygen/v3/lipsync/{precision,speed}` |
| First/last frame to video (AI transition) | `blackforestlabs/flux-3/first-last-frame-to-video`, `fal-ai/pika/v2.2/pikaframes` |
| Image gen/edit | `fal-ai/nano-banana-pro/edit`, `fal-ai/flux-2-flex`, `fal-ai/krea-2/turbo`, many more |

Several of these (background removal, interpolation, upscale) are also on
the ML-features agent's list as **local** features. The cloud versions are a
fallback for weak machines and a quality ceiling for strong ones. Both
should implement the same engine command (for example
`effects_remove_background`), with a "run on: local | fal" choice.

**Pricing.** Pay per output: video per second (examples: Kling Video v3
$0.14/s, MiniMax H3 Max $0.05/s), image per image or megapixel, audio per
1k characters. Each model has its own price. The pricing API makes an exact
estimate possible. The pricing page did not state a free credit amount.

**Output use.** fal's terms allow commercial use in general, but **each model
has its own licence**. Store the endpoint id with the result. Then the licence
can be looked up later, and a non-commercial model can be flagged.

**Verdict: integrate, second wave. This is the main Generate/Process
provider.** One integration and a curated model list per feature give
chukcut CapCut's AI tools (remove background, upscale, smooth slow-mo, AI
video, lip-sync). The user needs one key.

Sources: <https://fal.ai/docs/model-apis/model-endpoints/queue.md>,
<https://fal.ai/docs/documentation/model-apis/pricing.md>,
<https://fal.ai/pricing>, <https://fal.ai/explore/search?q=lipsync>,
<https://blog.fal.ai/pika-api-is-now-powered-by-fal/>

### 4.3 Replicate

**Auth.** `Authorization: Bearer <token>`.

**Shape.** `POST /v1/models/{owner}/{name}/predictions` (official models) or
`POST /v1/predictions` with a version (community). Async by default; poll the
prediction URL. Sync with `Prefer: wait` (default 60 s, or `wait=N`).
`Cancel-After` header (5 s – 24 h) for auto-cancel. Webhooks with
`webhook_events_filter`.

**Retention.** API inputs, outputs, files and logs are **deleted after 1 hour**.
Download at once.

**Limits.** 600 prediction creates/min, 3000 req/min elsewhere. An account
with free credit and no payment method gets 6 req/min.

**Pricing.** Public models: pay for active compute time (or per output on
official models). Failed runs are free. Output licence depends on each model.
The cost before running is often unknown for time-billed models.

**Verdict: later.** Same value as fal, weaker cost estimate, 1-hour
retention. Add it as a second "many models" provider if users ask.

Sources: <https://replicate.com/docs/topics/predictions/create-a-prediction>,
<https://replicate.com/docs/topics/predictions/rate-limits>,
<https://replicate.com/docs/topics/predictions/data-retention>,
<https://replicate.com/docs/topics/billing>

### 4.4 Runway

**Auth.** `Authorization: Bearer <key>` + `X-Runway-Version` header.
Self-serve keys from the developer portal.

**Shape.** Async task: `POST` to a generation endpoint, then
`GET /v1/tasks/{id}`. `DELETE /v1/tasks/{id}` cancels or deletes.
`GET /v1/organization` returns organisation info (use it for the test button).
Output URLs expire.

**Pricing.** Credits, $0.01 each. Examples: Gen-4.5 12 credits/s, Seedance
2.5 20–68 credits/s, WAN3 5–20 credits/s, image upscale 25–150 credits, video
upscale $0.007–0.012 per output frame, audio 0.25–5 credits/s. Self-serve
tiers set the limits.

**Verdict: later.** Strong models, but most of them are also on fal or
Higgsfield. Add it when a user needs a Runway-only model (Aleph video
editing, for example).

Sources: <https://docs.dev.runwayml.com/>,
<https://docs.dev.runwayml.com/guides/pricing/>,
<https://docs.dev.runwayml.com/api/>

### 4.5 Luma

**Auth.** Bearer key (now "Luma Agents API", base
`https://agents.lumalabs.ai/v1`). `POST /v1/generations`, poll
`GET /v1/generations/{id}`, download from a presigned URL. Models: Ray 3.2
(video), uni-1 / uni-1-max (image). The images take 30–60 s. For video, wait
30 s before the first poll and allow a 10-minute timeout. The API has an
optional `user_id` field. **chukcut must not fill it** (no identity in
outgoing requests).

**Verdict: later.**

Sources: <https://docs.lumalabs.ai/docs/api>, <https://docs.agents.lumalabs.ai/>

### 4.6 Kling (direct)

**Auth.** Since the move to `https://api-singapore.klingai.com`, an API key
from the Kling console as `Authorization: Bearer <key>` ("API Key
authentication must be used"). The older access-key/secret-key JWT scheme is
legacy. Async tasks with a query endpoint and a callback protocol.
Prepaid resource packs.

**Verdict: later.** Kling models are on fal and on Higgsfield, so a direct
integration adds only billing choice.

Source: <https://kling.ai/document-api/api/get-started/authentication>

### 4.7 Google Veo and Gemini image models (Gemini API)

**Auth.** `x-goog-api-key` (AI Studio key).

**Veo.** `veo-3.1-generate-preview` through `predictLongRunning`, then poll
the operation. 4, 6 or 8 s clips, 720p / 1080p / 4K, 16:9 or 9:16, 24 fps,
**native audio**. Latency 11 s to 6 min. Videos are kept on the server for
**2 days**. **SynthID watermark** on every video. Person generation is
restricted in EU/UK/CH/MENA. That matters for a user in Germany.

**Pricing.** Veo 3.1 standard $0.40/s (4K $0.60/s), Fast $0.10–0.30/s, Lite
$0.05–0.08/s. No free tier for Veo. Blocked generations are not charged. The
image model "Nano Banana 2" costs about $0.045–0.151 per image, depending on
size.

**Verdict: later.** One Gemini key would give TTS (free tier), images and Veo.
That is attractive, but Veo is also on fal (`fal-ai/veo3.1`).

Sources: <https://ai.google.dev/gemini-api/docs/veo>,
<https://ai.google.dev/gemini-api/docs/pricing>

### 4.8 OpenAI images (and Sora)

**Images.** `POST /v1/images/generations` and `/v1/images/edits`, base64 PNG
by default (JPEG/WebP optional), `background: "transparent"`, sizes such as
1024×1024 / 1536×1024 / 1024×1536. A generation can take up to 2 minutes.
**The organisation must pass "API Organization Verification"** before it can
use GPT Image models. That is an extra step for the user.

**Sora.** The Videos API (`/v1/videos`) is **deprecated as of 24 Sep 2026**.
Do not integrate it.

**Verdict: later** (images). A transparent PNG ("make a sticker of X") is the
one thing that would make it worth adding. Not possible for Sora.

Sources: <https://developers.openai.com/api/docs/guides/image-generation>,
<https://developers.openai.com/api/docs/guides/video-generation>

### 4.9 Pika

Pika's official API runs on fal (`fal-ai/pika/v2.2/text-to-video`,
`…/image-to-video`, `pikaframes`). There is no separate Pika key. **It comes
free with the fal integration.**

Source: <https://blog.fal.ai/pika-api-is-now-powered-by-fal/>

---

## 5. Transcription and translation

The captions agent already plans "OpenAI-compatible API with own base URL /
key / model, or local Whisper". That must become the first user of the
provider registry below, so that the key is stored once.

### 5.1 OpenAI-format transcription

`POST /v1/audio/transcriptions`, multipart, **25 MB file limit** (mp3, mp4,
mpeg, mpga, m4a, wav, webm). Extract the audio track and encode to Opus/MP3
before upload. Cut long audio at silences.

| Provider | OpenAI format? | Word timestamps | Price | Notes |
|---|---|---|---|---|
| OpenAI `gpt-transcribe` | yes (it is the reference) | **no** | $0.0045/min | best accuracy, prompt + keywords |
| OpenAI `whisper-1` | yes | **yes** (`verbose_json` + `timestamp_granularities[]=word`) | see pricing page | **use this for captions**, also translates to English and returns SRT/VTT |
| OpenAI `gpt-4o-transcribe-diarize` | yes | segments, with speaker labels | see pricing page | for interviews |
| **Groq** | **yes** (`https://api.groq.com/openai/v1`) | **yes** (segment or word) | `whisper-large-v3-turbo` **$0.04/h**, `whisper-large-v3` $0.111/h | 25 MB free tier, 100 MB dev tier; very fast |
| Deepgram | **no** (`POST /v1/listen`, `Authorization: Token <key>`) | yes (`words[]` with start/end/confidence) | Nova-3 $0.0043/min; **$200 free credit** | sync; diarisation; optional `callback` |
| AssemblyAI | **no** (`POST /v2/transcript`, then poll `GET /v2/transcript/{id}`) | yes (`words[]`) | Universal-3.5 Pro $0.21/h, Universal-2 $0.15/h; **$50 free credit** | async; EU endpoint `api.eu.assemblyai.com`; set `speech_models` explicitly |
| ElevenLabs Scribe | no (own API) | yes | $0.22/h | same key as TTS |

**Word timestamps are the requirement** for CapCut-style word-by-word
captions. In OpenAI format, only `whisper-1` and Groq's Whisper give them. A
generic "OpenAI-compatible" provider must therefore send
`response_format=verbose_json&timestamp_granularities[]=word` and handle a
server that ignores it. Without words in the answer, fall back to sentence
mode and say so in the UI.

**Verdict.** OpenAI-compatible (OpenAI, Groq, local whisper servers):
**integrate first**. The captions agent builds it. Deepgram and AssemblyAI:
later, as their own adapters. They are good, with large free credits, but
they do not use the OpenAI format.

Sources: <https://developers.openai.com/api/docs/guides/speech-to-text>,
<https://console.groq.com/docs/speech-to-text>,
<https://developers.deepgram.com/reference/speech-to-text/listen-pre-recorded>,
<https://deepgram.com/pricing>,
<https://www.assemblyai.com/docs/api-reference/transcripts/submit>,
<https://www.assemblyai.com/pricing>

### 5.2 DeepL for caption translation

**Auth.** `Authorization: DeepL-Auth-Key <key>`. Free keys end in `:fx` and go
to `https://api-free.deepl.com`. Pro keys go to `https://api.deepl.com`.
Choose the host from the key suffix. Do not ask the user.

**Request.** `POST /v2/translate`. Send **every caption line as its own
`text` value** in one request (128 KiB limit per request). Translations come
back in the same order, so caption timing stays the same. The `context`
parameter (the text before and after) improves the translation and is **not
billed**. `model_type`: `quality_optimized` / `latency_optimized`.
`tag_handling=xml` keeps inline style tags (karaoke highlight spans) intact.
`GET /v2/usage` is the test-connection call.

**Pricing (changed in 2026).** API Free is now a **one-time credit of 1 M
characters**, not a monthly allowance. API Pro: €23.80/month (billed
annually) including 50 M characters/month, then €22 per extra million.

**Alternative:** any OpenAI-compatible chat model can translate captions
too. The registry has the key already. DeepL is better at keeping line
boundaries.

**Verdict: integrate, second wave.**

Sources: <https://developers.deepl.com/docs/getting-started/auth>,
<https://developers.deepl.com/api-reference/translate>,
<https://www.deepl.com/en/pro#api>

---

## 6. Stock media with keys (brief; the open-assets survey goes deeper)

| Service | Auth | Limits | Licence for the user's video | Rules for the app |
|---|---|---|---|---|
| **Pexels** (photo + video) | `Authorization: <key>` | 200 req/h, 20k/month (more on request) | free to use | show "Photos provided by Pexels" + link; credit the photographer where possible; **do not copy Pexels' core functionality** (our reading: a search panel inside an editor is normal use; a Pexels clone is not) |
| **Pixabay** (photo + video, **no music**) | `key=` param | 100 req / 60 s | Pixabay Content License: commercial use, no attribution needed | **cache results for 24 h**; show where results come from; do not hotlink images permanently (download them) |
| **Freesound** (SFX) | token for search and **previews** (`preview-hq-mp3`); **OAuth2 for original files** | per-user throttling, numbers not published; `Usage` endpoint reports them | per-sound CC licence: CC0, CC-BY or **CC-BY-NC** | store each sound's licence; flag CC-BY (credit needed) and CC-BY-NC (no monetisation) |

**Freesound OAuth2 fits the "own key" rule.** Any user can create their own
API credentials at `https://freesound.org/apiv2/apply`. If the client uses
Freesound itself as the redirect, the authorisation `code` is "displayed on
screen so users can easily copy it". So chukcut asks the user for *their*
client id + secret, opens the browser, and the user pastes the code back.
No project registration and no local web server. The access token lasts 24 h
and refreshes with a refresh token.

Sources: <https://www.pexels.com/api/documentation/>,
<https://pixabay.com/api/docs/>,
<https://freesound.org/docs/api/authentication.html>,
<https://freesound.org/docs/api/resources_apiv2.html>

---

## 7. Integration architecture for chukcut

This follows the repository's rules: the engine has no UI dependency,
everything is a command in `modules/<name>/commands.rs`, document changes go
through `EditCommand`, and long work runs through `shell::spawn_blocking` and
a `shell::Channel`.

### 7.1 One new engine module: `modules/cloud/`

```
crates/engine/src/modules/cloud/
  mod.rs          what this module owns; the capability traits
  registry.rs     provider descriptors, the list of configured accounts
  secrets.rs      the key store (0600 file)
  http.rs         one HTTP client: neutral User-Agent, timeouts, redaction
  jobs.rs         the job queue: concurrency per provider, persistence, polling
  provenance.rs   source and licence metadata for every result
  providers/
    openai_compat.rs   TTS + transcription + chat (OpenAI, Groq, local servers)
    elevenlabs.rs
    fal.rs
    higgsfield.rs
    deepl.rs
    pexels.rs  pixabay.rs  freesound.rs
    stability.rs
  commands.rs     cloud_list_providers, cloud_set_key, cloud_test, cloud_estimate,
                  cloud_run, cloud_cancel, cloud_jobs …
```

**HTTP client.** The engine has no HTTP client yet and no async runtime. Use
a blocking client (`ureq` with rustls) on `spawn_blocking` threads. That fits
the existing shell primitives, and it does not bring in tokio. Every request
gets `User-Agent: chukcut/<version>` and nothing else that identifies the
user: no email, no account name, no hostname. Fields like Luma's `user_id` or
Epidemic's `x-partner-user-id` stay empty. Azure requires a User-Agent, and
the neutral one satisfies it.

### 7.2 The provider registry and the key store

**Descriptor (compiled in).** Each provider declares an id, a display name,
the capabilities it implements, the fields it needs (key; key + secret for
Higgsfield; key + region for Azure; base URL + key + model for
OpenAI-compatible; client id + secret for Freesound), a link to the vendor's
key page, a link to its terms, and a test call.

**Accounts (user data).** The user can configure the same provider type more
than once ("OpenAI", "Groq" and "Local Kokoro" are three OpenAI-compatible
accounts). Account settings go in the normal settings file. They have no
secrets, only a reference such as `secret_ref = "acct-7f3a"`.

**Secrets file.** `config_root()/secrets.toml` (`~/.config/chukcut/`),
created with mode **0600** in a directory with mode **0700**:

- Write to a temporary file in the same directory with mode 0600, `fsync`,
  then `rename` over the old file. A crash cannot leave a half-written or
  world-readable file.
- At load, check the mode. If the group or others can read the file, fix it
  and log a warning. Never refuse to start.
- **Never in project files, never in the cache, never in logs.** The project
  stores the provider id, account id, model and prompt, and never the key.
  A shared `.chukcut` file must not leak a key.
- The `tracing` layer redacts any header named `authorization`,
  `xi-api-key`, `x-goog-api-key`, `ocp-apim-subscription-key` or
  `DeepL-Auth-Key`. Error messages from a provider are logged, but request
  headers are not.
- The settings UI shows only the last 4 characters of a key.
- Later option: the Secret Service (libsecret) via a crate such as `oo7`,
  for users with a keyring. The 0600 file stays the default, because it
  works headless and over SSH, and the CLI/MCP shells need it.
- For the CLI and MCP shells: an environment variable per account
  (`CHUKCUT_KEY_<ACCOUNT>`) overrides the file and is never written back.

**Test connection.** One cheap, free call per provider. It shows "OK, plan:
Creator, 81 % credits left" or the vendor's own error text.

| Provider | Test call |
|---|---|
| ElevenLabs | `GET /v1/user/subscription` (tier, characters used/limit) |
| OpenAI-compatible | `GET {base}/models` (also fills the model dropdown) |
| fal | `GET api.fal.ai/v1/models/pricing?endpoint_id=<a default model>` |
| Runway | `GET /v1/organization` |
| DeepL | `GET /v2/usage` (characters used/limit) |
| Pexels / Pixabay | one search with `per_page=1` (the rate headers show the remaining quota) |
| Stability, Higgsfield, Replicate, Deepgram, AssemblyAI | an account or balance call. Find the exact endpoint when you implement each one. They were not checked in this survey. |

### 7.3 Capability traits

Group by **what the editor asks for**, not by vendor. A vendor implements
several traits. The UI asks the registry "who can do `Tts`?" and shows only
configured accounts.

| Trait | Input → output | Implementations (in order of value) |
|---|---|---|
| `Tts` | text + voice + options → audio file, **optional word timing** | ElevenLabs (with timing), OpenAI-compatible, Cartesia, Azure, Gemini |
| `VoiceList` | filter → voices with preview URLs | ElevenLabs, Cartesia, Azure |
| `VoiceChange` | audio + voice → audio | ElevenLabs |
| `Dub` | media + target language → audio (async) | ElevenLabs |
| `AudioGen` | prompt + duration (+ loop, instrumental) → audio | ElevenLabs SFX, ElevenLabs Music, Stable Audio |
| `MediaSearch` | query + kind (photo/video/sfx) → results with previews and a licence | Pexels, Pixabay, Freesound |
| `Generate` | prompt (+ image / first & last frame) → image or video | fal, Higgsfield, Runway, Luma, Gemini |
| `Process` | media in → media out (remove background, upscale, interpolate, lip-sync) | fal (later Higgsfield, Runway) |
| `Transcribe` | audio → words with times | OpenAI-compatible, Deepgram, AssemblyAI, ElevenLabs Scribe |
| `Translate` | caption lines + context → lines | DeepL, OpenAI-compatible chat |

The brief listed six traits. Two more are needed: `AudioGen` combines sound
effects and music, because they have the same shape (prompt + length →
audio). `Process` is separate from `Generate`, because it **uploads the
user's media**. The confirmation dialog must say so ("this sends 84 MB of
your clip to fal.ai").

Every trait method takes a request and returns a **job spec**. It does not
do the work itself. A job spec says how to submit, how to read progress, how
to fetch the result and how to cancel. Sync providers (ElevenLabs TTS) and
async providers (fal) then run in the same queue. For a sync provider the job
is one request. Sketch:

```rust
pub trait Tts {
    fn voices(&self, filter: &VoiceFilter) -> Result<Vec<Voice>>;
    fn plan(&self, req: &TtsRequest) -> Result<JobSpec>;   // no network
}

pub struct JobSpec {
    pub account: AccountId,
    pub estimate: CostEstimate,           // shown before running
    pub uploads: Vec<UploadNotice>,       // what leaves the machine
    pub steps: JobSteps,                  // Sync | Queue { poll, cancel } | Stream
    pub result_kind: MediaKind,
}
```

The fal and Higgsfield adapters take a **curated model table** per
capability (endpoint id, display name, input mapping, price unit). Do not
render arbitrary model forms. A short, tested list ("Remove background: Bria
v3 / BiRefNet v2"; "Upscale: SeedVR / Topaz precision") is what a CapCut user
expects. The table is data (TOML in the repo), so a model can be added
without a code change.

### 7.4 The job queue

- **Cost first.** `cloud_estimate` runs before `cloud_run`, and the UI shows
  the number. Exact where the vendor allows it: the fal pricing API, the
  Higgsfield estimate endpoint, Stability's flat credits, ElevenLabs
  characters × plan rate. Where it cannot be exact, show a range
  ("Replicate: billed by GPU time, usually $0.02–0.10") and say so. A
  per-account **spending confirmation threshold** (default: ask above $0.50)
  stops a slip of the finger on a $12 4K Veo job.
- **Concurrency per account**, from the descriptor, and editable (ElevenLabs
  Free = 2, Higgsfield = 4, …). Queue the extra jobs. On a 429, back off
  exponentially. Never run a tight retry loop.
- **Progress.** fal: the SSE status stream. Others: polling with backoff (2 s,
  then up to 15 s; Luma video: 30 s before the first poll). Progress goes to
  the UI through a `shell::Channel<JobEvent>`. The events are `Queued(pos)`,
  `Running(pct?)`, `Log(line)`, `Downloading(bytes)`, `Done(MediaId)` and
  `Failed(msg)`.
- **Persistence.** Write a small journal of open jobs (provider, account,
  request id, status URL, target project) under the state directory. If the
  app restarts while a $3 video job is in the queue, polling continues and
  the result still arrives. Jobs are not lost.
- **Download at once.** Result URLs expire (Replicate 1 h, ElevenLabs dubbing
  ~1 h, Veo 2 days, Higgsfield 7 days). Fetch the file the moment the job
  ends. Use `Idempotency-Key` where supported (Higgsfield), so a retried
  submit does not pay twice.
- **Cancel** where the vendor allows it (fal: always; Higgsfield: only while
  queued; Runway: `DELETE`). Show that limit in the UI.
- **No webhooks.** A desktop app has no public URL. Polling and SSE only.

### 7.5 Results become normal media, with provenance

A generated file is **user data that cost money**. It is not cache. The
`paths.rs` rule is "deleting the whole cache tree must never lose user
data", so results must **not** go under `cache_root()`.

- Saved project: `<project dir>/<project name> media/generated/<date>-<provider>-<short id>.<ext>`.
- Unsaved project: `$XDG_DATA_HOME/chukcut/generated/` (add a `data_root()` to
  `paths.rs`), and offer to move the files when the project is first saved.
- The import goes through the normal media import, so a generated clip
  becomes a material like any other clip, with thumbnails, a waveform and
  proxies.
- Each material gets an `origin` block in the project file, plus a
  `.json` sidecar next to the file, so the record survives if the file is
  used in another project:

```jsonc
"origin": {
  "kind": "generated",               // generated | stock | library
  "provider": "elevenlabs",
  "model": "eleven_v4",
  "endpoint": "/v1/text-to-speech",  // fal: the endpoint id
  "prompt": "…",                     // or voice + text for TTS
  "request_id": "…",
  "created_at": "2026-10-03T12:00:00Z",
  "cost": { "amount": 0.04, "unit": "USD", "estimated": false },
  "account_tier": "free",            // from the test call, at generation time
  "licence": {
    "commercial": "no",              // yes | no | unknown
    "attribution": null,             // "Photo by X on Pexels", "CC-BY: Y"
    "terms_url": "https://elevenlabs.io/pricing",
    "watermark": null                // "synthid" for Veo, "c2pa" if signed
  }
}
```

**Use the provenance.**

- The export dialog lists the assets on the timeline that have
  `commercial: no` or `unknown`. Examples: ElevenLabs free-tier audio, a
  CC-BY-NC Freesound effect, a non-commercial model on fal. The user then
  sees the claim risk before uploading, not after a strike.
- It also collects every `attribution` string into one credits text that
  the user can copy (Pexels, CC-BY sounds).
- The timing that ElevenLabs returns with a voiceover goes straight into the
  captions module as a word track. A voiceover then gets captions with no
  extra call.

### 7.6 Commands (the shell-facing API)

Following `<module>_<verb>`:

```
cloud_providers()                       -> descriptors + configured accounts
cloud_account_set(provider, fields)     -> AccountId      (writes secrets.toml)
cloud_account_remove(account)
cloud_account_test(account)             -> TestReport     (plan, quota, error)
cloud_voices(account, filter)           -> Vec<Voice>
cloud_search(account, query)            -> Vec<SearchHit> (stock media)
cloud_estimate(request)                 -> CostEstimate + UploadNotice
cloud_run(request, channel)             -> JobId          (events via Channel)
cloud_cancel(job)
cloud_jobs()                            -> open and recent jobs
```

The result arrives as a `Done(MediaId)` event. The UI then inserts it with a
normal `EditCommand` (for example "add clip at playhead"). The cloud module
never edits the document itself. The CLI and the MCP server get every
integration for free through the same commands.

---

## 8. Priority list

Ordered by value for the effort. Each wave assumes the one before.

1. **Foundation** — `modules/cloud`: registry, 0600 secrets file, HTTP client
   with redaction, job queue, provenance, export warning. Do this first,
   **together with the captions agent**. Its "OpenAI-compatible base URL /
   key / model" must be the first account type in this registry, not a
   private settings field.
2. **OpenAI-compatible Transcribe + TTS** (OpenAI, **Groq** for $0.04/h
   Whisper with word timestamps, local Kokoro/Whisper servers). One adapter,
   three use cases, and it bridges to fully offline TTS.
3. **ElevenLabs: TTS with timestamps, voice list with previews, sound
   effects.** This is CapCut's most used AI audio feature. Captions come free
   from the timing. SFX is sync and cheap. Read the tier for the licence
   flag.
4. **Stock search: Pexels + Pixabay (+ Freesound previews).** Free keys,
   simple sync APIs, an immediately useful media panel. Low effort.
5. **fal.ai `Process` + `Generate`:** video background removal, upscale,
   frame interpolation first. They make visible improvements to the user's
   own footage, and they share engine commands with the planned local ML
   features. Then image-to-video, and first/last-frame transitions. Lip-sync
   comes with dubbing (wave 8).
6. **DeepL caption translation** (and LLM translation through the
   OpenAI-compatible account). Small; makes captions multi-language.
7. **Music generation:** ElevenLabs Music + Stable Audio (2.5 sync, 3.0
   async). Also the **watched "music library" folder** that indexes tracks the
   user downloaded from Epidemic or Artlist. That is the realistic substitute
   for a licensed library.
8. **Higgsfield** as a second `Generate` provider (the owner's existing
   credits and characters), then ElevenLabs **voice changer and dubbing** +
   fal **lip-sync**: translate a talking-head video end to end.
9. **Later, on demand:** Runway, Luma, Kling direct, Gemini (Veo + TTS free
   tier + images), Replicate, Cartesia, Azure, Deepgram, AssemblyAI, OpenAI
   images (transparent stickers).
10. **Not possible; do not build:** Epidemic Sound, Artlist, Soundstripe,
    Musicbed, Uppbeat, Pixabay Music (partner-only or no API); Suno and Udio
    (no API; Udio has no downloads); PlayHT (shut down); OpenAI Sora
    (deprecated). Epidemic's ES Connect is the one exception to keep in
    mind. It needs no server, only a one-time app registration with
    Epidemic. If the owner ever accepts that one registration, it gives a
    real licensed library with the user's own subscription.

## Not verified

- Higgsfield: the path of the cost-estimate endpoint, whether API credits are
  the same pool as the web subscription, and the commercial-use terms for API
  output.
- fal: the free credit amount for new accounts, and the file-upload (storage)
  endpoint for `Process` inputs.
- Stability: whether the $1 M Community Licence threshold applies to API
  output or only to self-hosted models.
- Test/balance endpoints for Stability, Higgsfield, Replicate, Deepgram and
  AssemblyAI (§7.2).
- Freesound's numeric rate limits (not published; read them from the `Usage`
  endpoint at runtime).
- Google Cloud TTS API-key support is shown by third-party use, not by
  Google's authentication page.
- PlayHT's shutdown dates come from secondary sources, because the vendor's
  site no longer exists.
