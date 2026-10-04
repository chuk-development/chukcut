# Open assets: where chukcut's asset library can come from

Written 2026-10-03. Research only, no code. It answers one question: chukcut is
GPL-3.0 and open source, so where can its fonts, stickers, music, sound effects,
stock media, effects, transitions, LUTs and title templates come from legally?
It also asks which of these sources a CapCut-style asset panel can browse at
runtime, through an API.

Each claim has a source URL in the last section. "Measured" means that this
session sent the request and saw the response. "Not verified" means that this
session could not read the primary text, and the claim comes from a secondary
source. Terms change. Read the provider's page again before you build a provider.

## The answer in short

- **There is enough legal material for a good library, but most of it does not
  come from one API.** The best sources are open repositories (fonts, emoji,
  icons, transitions) and three stock APIs: Pexels, Pixabay and Freesound.
- **Music is the weak area.** No free music API gives tracks that a creator can
  safely put into a monetised video. The best path is a curated chukcut music
  pack from CC0, public-domain and CC BY tracks (Incompetech, Musopen recordings
  on Wikimedia Commons, Kenney), plus Jamendo and ccMixter as optional providers
  with strict licence filters.
- **Effects, transitions, LUTs and title templates should be ours.** gl-transitions
  (MIT) is a good base for transitions. Shadertoy and LYGIA are not usable. We
  generate LUTs and write title templates ourselves.
- **API keys:** Pexels and Pixabay accept a key per user. Freesound says that a
  key must stay secret. Unsplash says that an app must not make users register
  for developer keys. So "user brings their own key" works now. A small proxy
  (Cloudflare Worker, free tier is enough) makes the panel work without any user
  setup.
- **Avoid:** GIPHY (personal, non-commercial content, no caching), Tenor (shut
  down on 2026-06-30), BBC Sound Effects, YouTube Audio Library, Mixkit, Uppbeat,
  Bensound, Shadertoy and LYGIA.

## Ground rules that decide every verdict

### Three ways an asset can reach the user

| Way | What it means | Which licences allow it |
|---|---|---|
| **A. In the repository** | The file is committed to `chukcut` | Only code-like assets that we wrote or that have a GPL-compatible licence (MIT, BSD, Apache-2.0, ISC, OFL fonts as separate files). `CLAUDE.md` says that media never enters git. So no audio, video or image packs here. |
| **B. A chukcut asset pack** | A separate download that chukcut hosts (a GitHub release of a separate `chukcut-assets` repository, or a CDN). The app downloads it on first use. | CC0, public domain, CC BY, CC BY-SA, MIT, Apache-2.0, OFL. Not: any licence that forbids redistribution "on a standalone basis" (Pixabay, Pexels, Mixkit, Sonniss, Coverr, LottieFiles compilation). |
| **C. A runtime provider** | The app searches a third-party API. The user picks one item. The app downloads it into the project. | Anything that the provider's API terms allow for a desktop app. The licence of each item goes with the item. |

The Legal boundary in `CLAUDE.md` applies to all three: no ByteDance asset can
enter the repository, a pack or a provider list. A provider can never be
"CapCut's own CDN".

### What the licence of an asset does to the user's exported video

This matters more than the licence of the file. The user exports a video and
uploads it. The asset licence then controls the video.

- **CC BY**: the user must credit the author. chukcut must give the user the
  credit text (see "Credits file on export").
- **CC BY-SA with music**: CC 4.0 says that "Adapted Material is always produced
  where the Licensed Material is synched in timed relation with a moving image"
  (CC BY-SA 4.0 legal code, section 1a). So a CC BY-SA track in a video makes
  the video an adaptation, and the video must be CC BY-SA too. Most creators do
  not want that. Treat BY-SA music as "warn the user".
- **CC BY-SA with an image or emoji**: putting a sticker into a video is
  probably an adaptation too, but the legal position is less clear than for
  music. Treat it as "warn the user" (affects OpenMoji).
- **NC (NonCommercial)**: a monetised video, an advert or a sponsored post is
  commercial. Hide NC items by default. Show them only behind a filter, with a
  label.
- **ND (NoDerivatives)**: music in a video is an adaptation. ND music cannot be
  used at all. Never show ND audio.
- **Content ID**: a free licence does not stop a YouTube or Meta Content ID
  claim. Pixabay, Freesound and FMA all say that some of their tracks get
  claims. This costs the creator revenue, not a lawsuit, but it is a real cost.
  Store the licence proof (see "Local cache") so the user can dispute a claim.

### GPL and asset licences

Assets are data, not code. A GPL program can load assets under any licence, the
same way a GPL video player plays copyrighted films. The GPL does not "infect"
the assets, and the assets do not change the code licence. Only assets that we
commit to the repository need a licence that we are happy to ship next to GPL
code. `NOTICE.md` then lists them.

## Summary tables

Verdicts: **use** = build it, **care** = use with the stated condition,
**avoid** = do not build it.

### Fonts

| Source | Licence | API | Key | Verdict |
|---|---|---|---|---|
| **Fontsource API** | OFL-1.1 (2,056 of 2,100), Apache-2.0 (36), UFL (5), other (3). Measured. | Yes, JSON, file URLs on jsDelivr | No | **use** (primary catalogue) |
| **Google Fonts CSS2 API with `text=`** | Same fonts | Yes, CSS | No | **use** (preview tiles, about 9 KB each, measured) |
| google/fonts GitHub repository + `METADATA.pb` | OFL / Apache / UFL per directory | Git, no search | No | **use** (offline mirror, fallback) |
| Google Fonts Developer API | Same fonts | Yes, JSON | **Yes** | care (key; no reason to use it) |
| `fonts.google.com/metadata/fonts` | Same fonts | Undocumented JSON | No | care (has popularity; may change without notice) |
| Bunny Fonts `/list` | Same fonts | JSON + CSS | No | **use** (privacy-friendly mirror) |

### Stickers, emoji, icons, GIFs, animated stickers

| Source | Licence | API | Key | Verdict |
|---|---|---|---|---|
| **Microsoft Fluent Emoji** | MIT | Git / jsDelivr | No | **use** (pack; 3D style looks like CapCut stickers) |
| **Noto Emoji** (static) | Images Apache-2.0, font OFL-1.1 | Git | No | **use** (pack) |
| **Noto Animated Emoji** (881 Lottie) | CC BY 4.0 (not verified on Google's page) | Static JSON index + gstatic URLs | No | **use** (animated stickers, credit "Google") |
| Twemoji (jdecked fork) | Graphics CC BY 4.0, code MIT | Git / jsDelivr | No | **use** (pack, credit) |
| OpenMoji (4,495 entries) | Graphics CC BY-SA 4.0 | Git / jsDelivr | No | care (BY-SA, warn on export) |
| **Iconify API** (238 sets) | Per set, in API: MIT 108, CC BY 4.0 52, Apache 31, … 2 NC sets | Yes | No | **use** (filter sets by licence) |
| Lucide / Tabler | ISC / MIT | via Iconify | No | **use** |
| **Kenney** | CC0 | No (zip downloads) | No | **use** (pack) |
| Wikimedia Commons | Per file (PD, CC0, BY, BY-SA) | Yes (MediaWiki API) | No, but User-Agent policy | care (licence per file) |
| Openverse | Per item (CC, PD) | Yes | Optional | care (aggregator, licence not checked by them) |
| LottieFiles free animations | Lottie Simple License | No public API for third-party apps | – | care (manual import only) |
| Openclipart | CC0 (historically) | – | – | avoid (site did not answer in 20 s, measured) |
| GIPHY | Content owned by others; user terms: "personal and non-commercial" | Yes | Yes | **avoid** |
| Tenor | – | Shut down 2026-06-30 | – | **avoid** |

### Music

| Source | Licence | API | Key | Verdict |
|---|---|---|---|---|
| **Incompetech (Kevin MacLeod)** | CC BY 4.0, about 2,000 tracks | No | – | **use** (mirror a curated set into a pack, credit each track) |
| **Musopen recordings on Wikimedia Commons** | Public domain mark | Yes (Commons API) | No | **use** (classical only) |
| Jamendo | Per track CC; API "free for non-commercial uses" | Yes | Yes (client_id) | care (filter to BY / BY-SA / CC0; no offline caching by design) |
| ccMixter | Per track CC | Yes (Query API) | No | care (filter `lic=by`) |
| Openverse audio | Aggregates Jamendo, Freesound, Wikimedia audio | Yes | Optional | care |
| Free Music Archive | Per track CC | No (API page is 404) | – | avoid as provider |
| Pixabay Music | Pixabay Content License | No music API | – | avoid as provider (manual import is fine) |
| Mixkit music | Mixkit Free License | No; bots forbidden | – | avoid |
| YouTube Audio Library | YouTube-only licence for most tracks | No | – | avoid |
| Uppbeat, Bensound | Account-bound credit, free tier limits | No | – | avoid |

### Sound effects

| Source | Licence | API | Key | Verdict |
|---|---|---|---|---|
| **Freesound** | Per sound: CC0, CC BY 4.0, CC BY-NC 4.0 (+ legacy) | Yes, APIv2 | Yes, must stay secret | **use** (hide NC by default) |
| **Kenney audio packs** | CC0 | No | – | **use** (pack) |
| OpenGameArt | CC0, CC BY, CC BY-SA, GPL, OGA-BY | No | – | care (curate CC0 subset into a pack) |
| Sonniss GDC bundles | Royalty-free, no credit, no redistribution, no AI training | No | – | care (user's own local folder only) |
| Pixabay sound effects | Pixabay Content License | No | – | avoid as provider |
| BBC Sound Effects | RemArc licence: no money, no social media upload | No | – | **avoid** |

### Stock video and images

| Source | Licence of content | API | Key | Verdict |
|---|---|---|---|---|
| **Pexels** (photo + video) | Pexels License, no credit needed | Yes | Yes; 200/h, 20,000/month default | **use** (show "Pexels" link) |
| **Pixabay** (photo + video) | Pixabay Content License | Yes | Yes; 100 requests / 60 s | **use** (cache results 24 h) |
| Unsplash (photo) | Unsplash License | Yes | Yes; 50/h demo, 1,000/h production | care (needs proxy; hotlink thumbnails; trigger download endpoint) |
| Coverr (video) | Coverr licence, no AI training | Yes | Yes | care (logo credit; key process not verified) |
| Wikimedia Commons | Per file | Yes | No | care |
| Mixkit | Mixkit Free License | No; bots forbidden | – | avoid as provider |

### Effects, transitions, LUTs, titles

| Asset | Source | Licence | Verdict |
|---|---|---|---|
| Transitions | **gl-transitions** (125 shaders) | 123 MIT, 2 BSD (measured) | **use** (may enter the repository as code) |
| Effects | **Our own WGSL** | GPL-3.0 (ours) | **use** (primary) |
| Effects | ISF-Files (Vidvox, 371 shaders) | Repository MIT; per-file headers vary | care (filter per file) |
| Effects | Shadertoy | Default CC BY-NC-SA 3.0 | **avoid** |
| Effects | LYGIA | Prosperity 3.0 (non-commercial) or paid Patron | **avoid** |
| LUTs | **Generated by us** (procedural looks → `.cube`) | Ours | **use** (primary) |
| LUTs | Q-DDL 800 LUTs | CC BY 4.0 (secondary source) | care (check the licence file in the zip) |
| LUTs | RawTherapee HaldCLUT film simulation | CC BY-SA (not verified) | care (rename film-brand names) |
| LUTs | G'MIC CLUT packs | Mixed per pack (not verified) | care |
| Titles | **Our own JSON templates** | Ours | **use** (primary) |
| Titles | Lottie decorative layers (via velato or dotlottie-rs) | Per file | **use** for stickers and decoration, not for text |

## API keys: who holds the key

| Provider | Key needed | Can chukcut ship a key in the source? | Does each user need a key? | Recommendation |
|---|---|---|---|---|
| Fontsource, Bunny, Google CSS2, Iconify, Wikimedia, ccMixter, Openverse (anonymous) | No | – | No | Call directly |
| Pexels | Yes | Not forbidden, but the quota (200/h) is per key and would be shared | Possible (free account, instant key) | User key now; proxy later |
| Pixabay | Yes | Not forbidden; quota per key | Possible (free account) | User key now; proxy later |
| Freesound | Yes (token; OAuth2 only for original files) | **No**: "must be kept secret and confidential and under no circumstances be exposed to the public" | Possible (free account, apply for API credentials) | User key now; proxy later |
| Unsplash | Yes | No: "Keep your Access and Secret Keys confidential" | **No**: guidelines say do not make users register developer accounts | Proxy only |
| Jamendo | Yes (client_id) | Not stated; terms say non-commercial use only | Possible | User key (optional provider) |
| Coverr | Yes | Not verified | Not verified | Later |

**Prior art.** Kdenlive has the same feature ("Online Resources", since 2021).
It ships project keys for Freesound, Pexels and Pixabay in clear text in
`src/onlineresources/providermodel.cpp` ("registered with
online-resources@kdenlive.org"). It works in practice. But for Freesound it is
against the API terms quoted above, and every Kdenlive user shares one quota.
chukcut should not copy that.

**The proxy.** A Cloudflare Worker holds the keys and forwards search requests.
It returns the provider's JSON. Media files go directly from the provider's CDN
to the user, not through the proxy, so the proxy carries only small JSON.

- Cost: the Workers Free plan allows 100,000 requests per day. The Paid plan is
  $5 per month for 10 million requests. A search panel makes one request per
  search page. The free plan is enough until chukcut has thousands of daily
  users.
- The real limit is the provider's quota per key, not the proxy. Pexels gives
  higher limits on request when the app shows correct attribution. Unsplash
  needs a production review (1,000/h). Freesound negotiates commercial access
  case by case.
- The proxy must rate-limit per client IP, so one abuser cannot burn the shared
  quota.
- The proxy needs an owner account at each provider. That registration should
  use a project identity (a project e-mail and a project URL), not a personal
  one.

## Details: fonts

**Fontsource API** (`https://api.fontsource.org/v1/fonts`). No key. Hard limit
2,500 requests per 10 seconds, "constant usage at this rate can result in a
temporary ban". Measured on 2026-10-03: 2,100 families (1,980 from Google
Fonts, 120 others such as Adwaita Sans, Bravura, Clear Sans). Each family
object has `license` (SPDX), `category`, `weights`, `styles`, `subsets`,
`variable`, `unicodeRange`, and `variants` with per-weight, per-style,
per-subset URLs for `woff2`, `woff` and `ttf` on `cdn.jsdelivr.net/fontsource/`.
This is the best catalogue: it is complete, it has the licence, and it has TTF
URLs, which parley and skrifa read directly. The project asks for sponsorship
if you use the API.

**Google Fonts CSS2 API with `text=`.**
`https://fonts.googleapis.com/css2?family=Lobster&text=Lobster` returns a CSS
file that points to a font subset with only the glyphs of "Lobster". With a
non-browser User-Agent it is a TTF. Measured: 8,728 bytes. This is the right
way to draw CapCut-style preview tiles: each tile shows the family name in its
own face, and it costs about 9 KB, not the full font. No key. Bunny Fonts
supports `css2` too, but it returns whole subset files (measured), not a
`text=` subset.

**Google Fonts Developer API** needs an API key ("Your application needs to
identify itself every time it sends a request … by including an API key").
It gives the same data as Fontsource. There is no reason to use it.

**`https://fonts.google.com/metadata/fonts`** is undocumented. No key. Measured:
1,950 families, with `popularity`, `trending`, `designers`, `dateAdded` and
`isBrandFont`. Use it only to sort the font list by popularity, and treat a
failure as "no sort order".

**google/fonts on GitHub** has one directory per licence (`ofl/`, `apache/`,
`ufl/`) and one `METADATA.pb` per family. It is the source of truth and a good
offline fallback. It is large; do not clone it in the app.

**Licences.** OFL-1.1 allows use, embedding, bundling and redistribution, also
commercial. It forbids selling the font by itself. Reserved Font Names apply
only when you modify and redistribute the font. Text rendered into a video is
not a redistribution of the font. A shared project bundle that includes the
font files is a redistribution, and OFL allows it with the licence text. So
every cached font folder must keep its `OFL.txt`. Apache-2.0 and UFL-1.0 are
similar for our use.

**Privacy.** Google Fonts requests send the user's IP address to Google. Offer
Fontsource (jsDelivr) or Bunny Fonts as the default host, and Google CSS2 only
for preview subsets, or make the host a setting.

**Recommended:** catalogue from Fontsource, preview tiles from Google CSS2
`text=` (fallback: the Fontsource latin subset), full font download on first
use from Fontsource as TTF, cached under the user cache directory with the
licence file.

## Details: stickers, emoji, icons and GIFs

**Microsoft Fluent Emoji** (`github.com/microsoft/fluentui-emoji`). MIT
licence. Four styles: 3D, Color, Flat, High Contrast. PNG and SVG. The 3D
style is the closest free match to CapCut's sticker look. MIT allows a chukcut
pack with no credit needed beyond the licence file. Static only. Animated
Fluent emoji exist in Microsoft products, but not under this licence. Do not
use the third-party "animated Fluent" repositories.

**Noto Emoji** (`github.com/googlefonts/noto-emoji`). "Tools and most image
resources are under the Apache license, version 2.0", fonts under OFL-1.1, flag
images public domain. Use the SVGs in a pack.

**Noto Animated Emoji** (`googlefonts.github.io/noto-emoji-animation`).
Measured: the index `…/data/api.json` lists 881 animated emoji with codepoint,
tags, category and popularity. Each one is at
`https://fonts.gstatic.com/s/e/notoemoji/latest/<codepoint>/lottie.json`
(measured: 37 KB for 1f600), also as WebP and GIF. Licence: CC BY 4.0
according to Remotion's documentation, which wraps this set; this session
could not read the licence line on Google's page itself. These are the best
free animated stickers available. They are Lottie, so chukcut needs a Lottie
renderer (see "Titles"). Credit: "Animated emoji by Google, CC BY 4.0".

**Twemoji** (`github.com/jdecked/twemoji`, the maintained fork after X/Twitter
stopped). Version 17.0 (Unicode 17). Code MIT, graphics CC BY 4.0. The project
accepts "a mention in a project README or an 'About' section". For a video,
put the credit in the credits file.

**OpenMoji** (`github.com/hfg-gmuend/openmoji`). Measured: 4,495 entries in
`openmoji.json` (with skin-tone variants). Graphics CC BY-SA 4.0, code
LGPL-3.0. Suggested credit: "All emojis designed by OpenMoji – the open-source
emoji and icon project. License: CC BY-SA 4.0". The share-alike term is the
risk: a video with an OpenMoji sticker is probably an adaptation and then must
be CC BY-SA. Offer it, but label it and warn on export.

**Iconify API** (`https://api.iconify.design`). No key, "free to use", backup
hosts, can be self-hosted (Apache-2.0 server). Measured: `/collections` returns
238 icon sets, each with `license.spdx`: MIT 108, CC-BY-4.0 52, Apache-2.0 31,
OFL-1.1 13, CC0 10, CC-BY-SA 9, GPL variants 6, and two NC sets
(CC-BY-NC-SA-4.0, CC-BY-NC-4.0). Filter out NC and GPL sets for stickers, and
carry the set's licence with each icon. Monochrome icons are useful as simple
stickers and shapes. chukcut already uses Lucide (ISC) in the UI.

**Kenney** (`kenney.nl`). "All game assets on the asset pages are public domain
licensed (CC0). You're free to use them, even in commercial projects."
Attribution is not required. Do not use the Kenney logo. Good for shapes,
particles, UI elements and simple sound effects in a pack.

**Wikimedia Commons.** No key. The MediaWiki `imageinfo` API with
`iiprop=extmetadata` returns `LicenseShortName`, `Artist`, `AttributionRequired`
and `UsageTerms` per file. Commons has no fair-use files, but licences range
from public domain to CC BY-SA, and personality and trademark rights still
apply. The User-Agent policy wants a descriptive agent "with contact
information", and generic agents "may be blocked". This conflicts with the
owner's rule that no personal contact data goes into requests. Solution: send
`chukcut/<version> (<project URL>)` with a project URL that is not a personal
handle, for example a project organisation on GitHub or a project domain. This
needs an owner decision before the Commons provider ships.

**Openverse** (`api.openverse.org`, run by the WordPress Foundation). Measured:
anonymous access works, with rate-limit headers of 20 per minute burst and 200
per day sustained. Registration gives higher limits. It aggregates 536 million
Flickr images, 89 million Wikimedia images, and for audio 3.98 million
Wikimedia, 645,000 Jamendo and 591,000 Freesound items, with normalised licence
fields (`license`, `license_version`, `creator`, `foreign_landing_url`). Terms:
"Openverse … does not verify its licensing status", users must check licences
and give attribution, and an app "must prominently indicate that it was made
using Openverse but is not endorsed or certified by Openverse". No scraping, no
using several machines to bypass limits. 200 requests per day per IP is enough
for one person browsing. Good as one provider that covers many sources.

**LottieFiles.** The Lottie Simple License allows use, modification and
distribution, "including for commercial purposes", and attribution is "strongly
encouraged" but not required. But: "This license does not include the right to
collect or compile Files from LottieFiles to replicate or develop a similar or
competing service." A LottieFiles browser inside chukcut is close to that line.
There is a GraphQL API (exposed through their MCP server), but no published
terms that allow a third-party app to browse the catalogue. Verdict: allow the
user to import a `.json` or `.lottie` file that they downloaded. Do not build a
LottieFiles provider without written permission.

**GIPHY.** Beta keys allow 100 calls per hour. The API requires "Powered By
GIPHY" marks, and "GIPHY media should be loaded directly from the media URLs
returned by the API and should not be cached, proxied, rewritten, or stored".
A video editor must store the GIF to render it, so it breaks this rule at once.
Also, GIPHY's user terms say content may be used "solely for personal and
non-commercial purposes", and much GIPHY content is clipped from films and TV.
For a creator this means Content ID claims and takedowns. **Avoid.**

**Tenor.** Google stopped new API sign-ups on 2026-01-13 and shut the API down
on 2026-06-30: "any attempt to make an API request will fail". **Avoid.**

**Openclipart.** Did not answer within 20 seconds (2026-10-03). It has a long
history of downtime. Do not depend on it at runtime. If it is back, its CC0
content could go into a pack.

## Details: music

**The core problem.** A creator wants music that is (1) good, (2) free for
commercial and monetised use, (3) safe from Content ID. No free API gives all
three. So the music tab needs a curated chukcut pack as the default, and
providers as extras.

**Incompetech (Kevin MacLeod).** About 2,000 tracks, CC BY 4.0. The usual
credit is: `"<Track>" Kevin MacLeod (incompetech.com), Licensed under Creative
Commons: By Attribution 4.0 License, http://creativecommons.org/licenses/by/4.0/`.
A paid licence removes the credit. No API. CC BY allows redistribution, so
chukcut may mirror a curated set (for example 100 tracks by mood) into its own
pack, with the exact credit stored next to each file. Risk: these tracks are
very widely used, and wrong Content ID claims by third parties happen. Not
verified on the primary site in this session: the exact version and wording
(the licences page did not show it to the fetch tool; itch.io copies and a
licence guide agree on CC BY 4.0).

**Musopen.** The site is behind a Cloudflare challenge, so this session could
not read it. Secondary sources say the recordings are released to the public
domain, that Musopen asks users not to sell the recordings directly and to
credit Musopen, and that the free account allows 5 downloads per day. No API.
Many Musopen recordings are on Wikimedia Commons with the Public Domain Mark.
So use the Commons API to fetch them, and credit "Musopen" anyway. Classical
music only.

**Jamendo.** About 500,000 tracks according to its docs; Openverse indexes
645,000. API terms: "The API may be used freely for non-commercial uses. For
any other type of use including but not limited to commercial uses please
contact our sales team". Apps "must not be specifically designed to cache the
content nor offering an offline access to the content". Apps must credit the
artist and Jamendo and link to each track's page. Each track has its own CC
licence, and many are NC or ND. A chukcut provider is a free, non-commercial
app, which fits the API terms. But the user's video may be commercial, and
the no-offline rule conflicts with copying a track into a project. Verdict:
optional provider, show only CC BY, CC BY-SA (with warning) and CC0 tracks,
link to Jamendo Licensing for the rest. Quota: "JAMENDO reserves the right to
impose restrictions"; this session found no number.

**ccMixter.** Query API at `ccmixter.org/api/query`, no key, `f=json`, licence
filter `lic=by` (also `nc`, `sa`, `pd`). Still active. Mostly remixes and
vocals; quality varies. Use the `lic=by` filter only.

**Free Music Archive.** Now owned by Tribe of Noise. `freemusicarchive.org/api`
returns 404 (2026-10-03). The FAQ says CC BY, BY-SA, BY-NC and BY-NC-SA tracks
may go into videos, ND may not, and that some artists use Content ID. No
provider; users can import files they download by hand.

**Pixabay Music.** Allowed in videos without credit under the Pixabay Content
License. No music endpoint in the API (the API covers photos, illustrations,
vectors and videos). The licence forbids distribution "on a Standalone basis",
so chukcut cannot mirror it. Pixabay says that some composers' tracks are
detected by Content ID and offers a licence certificate to dispute. Manual
import only.

**YouTube Audio Library.** No API. Most tracks have a YouTube-only licence;
some are CC BY. **Avoid.**

**Uppbeat, Bensound.** No public API. Free tiers tie the credit to the user's
account (Uppbeat: 3 downloads per month and a credit link in the video
description). **Avoid** as providers.

**Mixkit music.** Free licence, no credit. But the Mixkit terms forbid using
"scripts or bots to mass download Items", forbid making Items available "on a
stock or inventory basis", and forbid building "a similar or competitive
product or service". No API. **Avoid** as provider.

## Details: sound effects

**Freesound** (Music Technology Group, UPF Barcelona). APIv2. Token
authentication is enough for search and for the HQ preview files (MP3 and OGG).
OAuth2 is needed only for the original upload file. OAuth2 works for desktop
apps: Freesound shows the code on screen for the user to paste. Every OAuth2
user needs a Freesound account. Limits: 60 requests per minute and 2,000 per
day; downloads of originals 30 per minute and 500 per day. Keys "must be kept
secret". Commercial API use is "negotiated on a case by case basis". Each sound
has one licence: CC0, CC BY 4.0, CC BY-NC 4.0 (Sampling+ is being retired).
Credit format from the FAQ: `"Sound Title" by username
(freesound.org/s/<id>/) licensed under CC BY 4.0`. For short sound effects in
social video, the HQ preview is good enough, which removes the need for OAuth2.
Verdict: **use**. Default filter: CC0 and CC BY only. Ask for the user's own
token in settings until a proxy exists.

**Kenney audio** (UI sounds, impacts, interface clicks). CC0. Put it into the
starter pack.

**OpenGameArt.** Licences CC0, CC BY 3.0/4.0, CC BY-SA 3.0/4.0, GPL 2/3 and
OGA-BY. Suggested credit: `"[asset name]" by [author] licensed [licence]:
[url]`. No API. Curate CC0 sound effects into the pack by hand.

**Sonniss GDC Game Audio Bundles.** Royalty-free, no credit, commercial use
allowed. But: "Not as standalone files or in sound effect libraries", and "Use
for AI/ML training is strictly prohibited". chukcut cannot host them. A user who
downloaded them can add their folder as a local library (see design below).

**Pixabay sound effects.** Same licence as Pixabay music, no API. Manual import.

**BBC Sound Effects.** The RemArc licence forbids "making money from our
content" and "sharing our content. For example, no uploading to social media
sites". A creator's video breaks both. **Avoid.**

## Details: stock video and images

**Pexels.** Photos and videos. Key from any Pexels account, instantly. Default
limit "200 requests per hour and 20,000 requests per month"; higher limits on
request when attribution is correct. API rules: "show a prominent link to
Pexels", credit photographers "when possible" ("Photo by [Name] on Pexels"),
do not "copy or replicate core functionality of Pexels". Content licence:
credit not required; do not sell unaltered copies; do not redistribute on other
stock platforms. Pexels terms forbid scraping "for machine learning purposes".
Verdict: **use**.

**Pixabay.** Photos, illustrations, vectors, videos. Key required. "Up to 100
requests per 60 seconds". "Requests must be cached for 24 hours." "Permanent
hotlinking of images … is not allowed": download the file to local storage.
"Show your users where the images and videos are from, whenever search results
are displayed." "Systematic mass downloads are not allowed." Maximum 500
results per query. Content licence: no credit required; no "Standalone"
distribution. Verdict: **use**. A desktop app that caches search results for
24 hours and downloads chosen files fits these rules well.

**Unsplash.** Photos only. Demo 50 requests per hour, production 1,000 per hour
after review. Rules: use the hotlinked `photo.urls` for display; call
`photo.links.download_location` when the user selects a photo; credit the
photographer and Unsplash with links that carry
`?utm_source=<app>&utm_medium=referral`; do not use "Unsplash" in the app name;
do not "require users to register for developer accounts" (use a proxy
instead). For a video editor: hotlink the thumbnails in the panel, call the
download endpoint and download the full file when the user adds the photo.
Verdict: **care**, phase 2, proxy only.

**Coverr.** Video. Public API with search and vertical-video filters. Free if
you do not resell. Apps must show "where the videos are pulled from" with the
Coverr logo as a link. Download URLs are signed and valid 15 minutes. Licence:
commercial use, no credit required, no reselling, no competing service, and
"must not be used to train AI algorithms". This session did not verify how to
get a key or the rate limits. Verdict: **care**, later.

**Mixkit.** Good free video, but no API and bots forbidden (see Music).
**Avoid** as provider.

**Wikimedia Commons.** Also has video (WebM) and many public-domain films. See
the stickers section for the API and the User-Agent issue.

## Details: effects and transitions

**gl-transitions.** Measured: `gl-transitions.json` on npm (via jsDelivr) has
125 transitions: 123 MIT, 1 BSD-3-Clause, 1 BSD-2-Clause. Each entry has
`name`, `glsl`, `author`, `license`, `paramsTypes` and `defaultParams`. The
GLSL uses `getFromColor(uv)`, `getToColor(uv)`, `progress` and `ratio`. MIT and
BSD are GPL-compatible, so these may enter the repository as source code, with
the author and licence kept in each file and listed in `NOTICE.md`. They go
through the existing GLSL path in `modules/effects/` (rewriter, glslang, naga),
or we port the good ones to WGSL by hand. **Use.**

**ISF-Files** (`github.com/Vidvox/ISF-Files`). Repository licence MIT, last
push 2026-06. Measured on a clone: 371 `.fs` shaders. Most are credited to
VIDVOX. But 6 files mention Shadertoy as their origin, and 1 file states
"Creative Commons Attribution-NonCommercial-ShareAlike 3.0". Some credit Inigo
Quilez, whose Shadertoy work is NC by default. So the repository licence does
not cover every file. A vidvox forum thread is titled "User bennoH is stealing
shaders and uploading them online with MIT license", which shows the problem.
**Care:** import file by file, keep only files that are clearly by VIDVOX or
carry an explicit MIT/BSD header.

**Shadertoy.** Shaders are "by default protected under a Creative Commons
Attribution-NonCommercial-ShareAlike 3.0 Unported" licence unless the author
says otherwise (shadertoy.com/terms; the page blocks automated fetches, the
quote is from secondary sources). NC makes them unusable for creators who earn
money. **Avoid**, except single shaders with an explicit MIT/CC0 header from
the author.

**LYGIA** (shader function library). Licence file: The Prosperity Public
License 3.0.0, "use and share this software for noncommercial purposes for free
and … try this software for commercial purposes for thirty days". A paid Patron
licence exists for commercial use. A non-commercial clause is not compatible
with GPL-3.0 (the GPL forbids extra restrictions). **Avoid**; do not copy
functions from it.

**Our own WGSL.** The primary source for effects. It is the only source with no
licence question, and it fits the wgpu pipeline without the GLSL rewriter.
Classic techniques (blur, glitch, chromatic aberration, VHS, film grain, zoom
blur, light leaks as procedural gradients) are not protected; specific code is.
Write them from the papers and descriptions, not from Shadertoy code.

## Details: LUTs and filters

**Generate our own looks.** This is the recommended path. A "look" is a small
parametric function (white balance, contrast curve, split toning, saturation per
hue, fade, grain is separate). chukcut evaluates it on a 33×33×33 grid and
writes a `.cube` file. Benefits: no licence question, unlimited presets, and
the parameters stay editable, so a user can tweak a preset in the inspector.
The current Filters tab (`crates/app/src/editor/assets/library.rs`) already
does this in a simple form with four parameters.

**The `.cube` format** (Adobe Cube LUT Specification 1.0, also used by
DaVinci Resolve). A text file. Keywords: `TITLE "…"`, `LUT_1D_SIZE N` or
`LUT_3D_SIZE N` (2–256; 33 and 65 are common), optional `DOMAIN_MIN r g b` and
`DOMAIN_MAX r g b` (default 0 and 1). Then N³ lines of `R G B` floats, with red
changing fastest, then green, then blue. Comments start with `#`. Resolve
writes `LUT_1D_INPUT_RANGE` / `LUT_3D_INPUT_RANGE` as its own extension; a
reader should accept and ignore unknown keywords. A LUT has no colour-space
tag, so the import dialog must ask (or assume Rec.709 gamma 2.4 display-referred
input, which is what most free LUTs expect).

**Free LUT packs** (user import, or a pack only if the licence file confirms):

- **Q-DDL, 800+ LUTs**, `.cube`, CC BY 4.0, according to CG Channel and
  CGPress (2020). The download site "include[s] popups and adverts". This
  session could not open it. Check the licence file in the zip before a pack.
- **RawTherapee film simulation HaldCLUTs** (Pat David and others, 2015, about
  `rawtherapee.com/shared/HaldCLUT.zip`). Reported as CC BY-SA; this session
  could not open the RawPedia page (404). HaldCLUT PNGs convert to `.cube`
  losslessly. The names are film brand names (Kodak, Fuji): rename them in a
  pack.
- **G'MIC CLUT packs**, 1,100+ CLUTs in `.png` and `.cube`, from many authors
  with per-pack terms. Not verified. G'MIC itself is CeCILL.
- **spektrafilm**, spectral film emulation by a pixls.us member; the author
  proposed CC BY-SA 4.0. Alpha. Watch it.
- FreeVisuals and similar "free LUT" sites usually allow commercial use but do
  not clearly allow redistribution. User import only.

## Details: title templates and animations

**Our own JSON templates.** A title template is a document fragment: one or
more text segments with style (font, size, colour, stroke, shadow, box) and
keyframed transforms and opacity. It uses the engine's own text module and the
project format, so it is undoable, editable and resolution-independent. Store
templates as files in a template pack, each with a `licence` field (ours: CC0,
so users can do anything with the result). Fonts are referenced by Fontsource
`id`, so a template can fetch its font on first use.

**Motion presets.** CapCut's "In", "Out" and "Loop" animations are parametric
keyframe recipes: fade, slide, zoom, pop with overshoot, typewriter, bounce,
shake. Each is a function from (duration, strength) to keyframes. They are
ours, they need no assets, and they apply to text, stickers and clips alike.
Write them as engine commands that emit `EditCommand`s.

**Lottie for decorative motion.** Lottie is the right format for animated
stickers, emphasis shapes, arrows, bursts and lower-third decorations. It is not
the right format for the text itself:

- **velato** (linebender, Apache-2.0/MIT, active) renders Lottie into a vello
  scene. Vello runs on wgpu and accepts an existing device, so it can use
  `gpu::render_context()` and respect the "one GPU device" rule. Missing: text
  layers, embedded images, stroke dashes, some effects.
- **dotlottie-rs** (LottieFiles, MIT, active) wraps ThorVG (MIT), a CPU
  renderer with wider Lottie coverage, including text. It would return RGBA
  frames like the current text layer.
- rlottie (Samsung) has a mixed licence (GitHub shows NOASSERTION); skip it.

Keep text in our own text engine, and put Lottie under it or around it. Then
velato's missing text support does not matter. Noto Animated Emoji are a good
first test corpus.

**Authoring.** Glaxnimate (KDE, GPL-3.0) edits and exports Lottie. It is the
natural tool for making our own animated stickers and decorations.

## Recommended design: chukcut's online library

### Shape

```
engine/src/modules/library/
  mod.rs          Provider trait, AssetRecord, licence model
  commands.rs     library_search, library_fetch, library_credits, library_providers
  providers/      one file per provider: fontsource, iconify, pexels, pixabay,
                  freesound, openverse, commons, jamendo, ccmixter, pack, local_folder
  cache.rs        content-addressed store + sidecar metadata
  licence.rs      SPDX ids, policy (allowed / warn / hidden), credit text
```

This follows the `CLAUDE.md` rules: every capability is a command, the engine has
no UI dependency, and a CLI or MCP server can search and fetch assets the same
way the panel does.

### Provider plugins

A provider is a Rust type that implements one trait:

```rust
trait Provider {
    fn id(&self) -> &'static str;                  // "pexels"
    fn kinds(&self) -> &[AssetKind];               // Video, Image, Music, Sfx, Font, Sticker, Lottie
    fn needs_key(&self) -> KeyNeed;                // None | UserKey | ProxyOrUserKey | OAuth
    async fn search(&self, q: &Query) -> Result<Page<Hit>, String>;
    async fn fetch(&self, hit: &Hit, dest: &Path) -> Result<AssetRecord, String>;
    fn panel_attribution(&self) -> Option<Attribution>; // "Photos provided by Pexels", logo, link
}
```

Notes on the design:

- **Rust, not JSON configuration.** Kdenlive describes providers in JSON. That is
  flexible, but the rules differ per provider in ways JSON cannot express:
  Unsplash's download-tracking call, Pixabay's 24-hour cache, Freesound's
  preview-versus-original split, Jamendo's licence filter. Write each provider in
  Rust. There will be about ten.
- **A licence policy layer between provider and panel.** Each `Hit` carries a
  normalised licence (SPDX id or a provider licence id such as
  `LicenseRef-Pexels`). The policy decides: show, show with a warning badge
  (BY-SA, Content ID risk), or hide (NC, ND, unknown). The user can show NC items
  through a filter, never ND audio.
- **Rate limits in the provider.** Each provider has a token bucket that
  matches the provider's published limits. The panel debounces typing (about
  400 ms) so a search does not fire per keystroke.
- **Search result cache.** Store results for 24 hours (Pixabay requires it; it
  is also polite for everyone else).
- **User-Agent.** `chukcut/<version>` with no personal data. For Wikimedia, add
  a project URL that is not a personal handle (owner decision, see above).

### API keys in settings

- Settings has one row per provider: status (ready / needs key / using
  proxy), a "Get a free key" link to the provider's key page, and a key field.
- Store keys in the system keyring (Secret Service on Linux, for example through
  the `keyring` crate), not in the project and not in a plain settings file.
  Never write a key into a log, an error message or a project file.
- Phase 1: keyless providers work at once (fonts, Iconify, Openverse, Commons,
  ccMixter, packs). Pexels, Pixabay, Freesound and Jamendo work after the user
  pastes a key.
- Phase 2: a small proxy (Cloudflare Worker) for Pexels, Pixabay, Freesound
  and Unsplash. The app uses the proxy when the user has no key, and the user's
  own key when they have one. The proxy URL is a setting, so a fork or a
  distribution can run its own.

### Local cache with licence metadata next to each asset

```
~/.cache/chukcut/library/
  <provider>/<id>/
    asset.<ext>              the file as downloaded (original or preview quality)
    asset.json               the sidecar, below
    LICENSE.txt              the licence text, when the licence asks for it (OFL, CC BY-SA)
```

The sidecar (`asset.json`):

```json
{
  "provider": "freesound",
  "id": "401275",
  "title": "Rain, Moderate, C.wav",
  "kind": "sfx",
  "creator": "InspectorJ",
  "creator_url": "https://freesound.org/people/InspectorJ",
  "source_url": "https://freesound.org/s/401275/",
  "licence": "CC-BY-4.0",
  "licence_url": "https://creativecommons.org/licenses/by/4.0/",
  "attribution_required": true,
  "credit": "\"Rain, Moderate, C.wav\" by InspectorJ (freesound.org/s/401275/) licensed under CC BY 4.0",
  "warnings": ["content-id-possible"],
  "fetched_at": "2026-10-03T12:00:00Z",
  "sha256": "…",
  "provider_terms_snapshot": "https://freesound.org/help/tos_api/"
}
```

- The sidecar is the user's proof for a Content ID dispute. Keep it even after
  the asset leaves the cache. Show a "Licence" button on every library item and
  on every timeline segment that came from the library.
- When a library asset goes into a project, the project records the sidecar
  data in the media entry (not only a path into the cache). A project then keeps
  its credits even if the cache is cleared or the project moves to another
  machine. "Collect project files" copies the asset and its sidecar.
- The cache is content-addressed by `sha256` so one file used in five projects
  is stored once. Size limit and eviction: least recently used, never an asset
  that an open project uses.
- Unsplash thumbnails are hotlinked and not cached, as its rules say. Full
  photos are downloaded on use, after the download-tracking call.

### Credits file on export

- On export, the engine walks the timeline and collects every segment whose
  media has a library sidecar with `attribution_required`, plus every provider
  that requires a panel or credit mention (Pexels, Unsplash, Openverse, Jamendo).
- It writes `<export-name>.credits.txt` next to the video, with one credit line
  per asset in the format the source asks for, grouped by music, sound,
  footage, images, stickers, fonts. It also offers "Copy credits" for pasting
  into a YouTube or TikTok description.
- The export dialog shows a short licence summary before export: "3 items need
  credit", "1 item is CC BY-SA: your video must be CC BY-SA", "1 item is
  NonCommercial: do not monetise". This is the place where a licence problem
  costs a click, not a takedown.
- Fonts (OFL) need no credit in the video. They appear in the credits file only
  if the user asks for a full list.

### Packs (way B)

- A separate repository (for example `chukcut-assets`) holds pack manifests and
  the files, published as release archives. The main repository holds only the
  URLs and hashes, which matches the "media never enters git" rule and the
  Legal boundary.
- Starter pack contents, all redistributable: Fluent Emoji 3D (MIT), Noto Emoji
  (Apache-2.0), Noto Animated Emoji (CC BY 4.0, if the licence is confirmed),
  Kenney shapes and UI sounds (CC0), curated OpenGameArt CC0 sound effects,
  about 100 Incompetech tracks (CC BY 4.0), public-domain classical from
  Musopen via Commons, our generated LUT looks, our title templates and motion
  presets, gl-transitions (MIT).
- Each pack file has the same sidecar as a provider download, so packs and
  providers share the credits logic.

### Local folders

A "local library" provider indexes a folder that the user picks (a Sonniss
bundle, a purchased LUT pack, a music subscription download). The user says
which licence applies to the folder, or "my own / licensed elsewhere". This
covers every source that chukcut cannot host or browse.

## Build order

1. Licence model, sidecar, cache and credits export (no provider works safely
   without them).
2. Keyless providers: Fontsource fonts with Google CSS2 preview tiles, Iconify,
   packs, local folders.
3. Starter pack: emoji, Kenney, gl-transitions, generated LUTs, title templates.
4. Key providers with user keys: Pexels, Pixabay, Freesound.
5. Openverse and Wikimedia Commons (after the User-Agent decision).
6. Lottie rendering (velato or dotlottie-rs) and Noto Animated Emoji.
7. Proxy for keyless use of Pexels, Pixabay, Freesound; then Unsplash.
8. Optional music providers: Jamendo, ccMixter.

## Open questions and things not verified

- **Owner decision:** the project identity for provider registrations and for
  the Wikimedia User-Agent (a project e-mail and URL, not a personal one).
- Noto Animated Emoji licence: **verified 2026-10-04** on Google's own page
  (googlefonts.github.io/noto-emoji-animation, Documentation, FAQ "Can I use
  these animated assets commercially"): "Animated Noto Emoji is licensed under
  CC BY 4.0". The page is a JavaScript app; read it in a browser.
- Incompetech: CC BY 4.0 per secondary sources; the primary page did not show
  the version.
- Musopen: site blocked by Cloudflare; licence from secondary sources.
- RawTherapee HaldCLUT and Q-DDL licences: secondary sources only. Read the
  licence file inside each archive before packing it.
- Coverr: key process and rate limits not found.
- Jamendo: no published quota; whether "no offline access" allows copying a
  track into a project is not clear. Ask Jamendo before shipping the provider.
- Unsplash: whether the API returns Unsplash+ (paid) photos that must be
  filtered out was not checked.
- Freesound: whether an open-source, free app counts as "non-commercial" API
  use is not defined in the terms. Kdenlive's practice suggests that Freesound
  tolerates it, but that is not permission.
- GIPHY: the full API Terms page redirected to the user terms; the API doc
  quotes above are from the developer documentation.
- Shadertoy default licence: the terms page blocks automated access; the
  CC BY-NC-SA 3.0 default is widely documented and was confirmed by several
  secondary sources.

## Sources

Fonts
- Google Fonts Developer API: https://developers.google.com/fonts/docs/developer_api
- google/fonts repository: https://github.com/google/fonts
- Fontsource API: https://fontsource.org/docs/api/introduction , https://fontsource.org/docs/api/fonts , https://api.fontsource.org/v1/fonts (measured)
- Google Fonts CSS2 with `text=`: https://fonts.googleapis.com/css2?family=Lobster&text=Lobster (measured)
- Google Fonts metadata (undocumented): https://fonts.google.com/metadata/fonts (measured)
- Bunny Fonts: https://fonts.bunny.net/about , https://fonts.bunny.net/list (measured)

Stickers, emoji, icons, GIFs
- OpenMoji: https://github.com/hfg-gmuend/openmoji , https://cdn.jsdelivr.net/npm/openmoji/data/openmoji.json (measured)
- Twemoji: https://github.com/jdecked/twemoji
- Noto Emoji: https://github.com/googlefonts/noto-emoji
- Noto Animated Emoji: https://googlefonts.github.io/noto-emoji-animation/ , https://googlefonts.github.io/noto-emoji-animation/data/api.json (measured), licence per https://convert.remotion.dev/docs/animated-emoji
- Fluent Emoji: https://github.com/microsoft/fluentui-emoji
- Iconify API: https://iconify.design/docs/api/ , https://api.iconify.design/collections (measured)
- Kenney: https://kenney.nl/support
- Lottie Simple License: https://lottiefiles.com/page/license
- LottieFiles MCP / GraphQL: https://docs.lottiefiles.com/en/platform/mcp
- GIPHY API: https://developers.giphy.com/docs/api/ ; user terms: https://support.giphy.com/hc/en-us/articles/360020027752-GIPHY-User-Terms-of-Service
- Tenor shutdown: https://developers.google.com/tenor/guides/quickstart , https://support.google.com/tenor/answer/10455265
- Wikimedia: https://www.mediawiki.org/wiki/API:Imageinfo , https://foundation.wikimedia.org/wiki/Policy:Wikimedia_Foundation_User-Agent_Policy , https://commons.wikimedia.org/wiki/Commons:Reusing_content_outside_Wikimedia
- Openverse: https://api.openverse.org/v1/ (measured), https://docs.openverse.org/terms_of_service.html , https://docs.openverse.org/api/reference/made_with_ov.html

Music
- Jamendo: https://developer.jamendo.com/v3.0/docs , https://devportal.jamendo.com/api_terms_of_use
- Free Music Archive FAQ: https://freemusicarchive.org/faq/
- ccMixter Query API: http://ccmixter.org/query-api
- Incompetech: https://incompetech.com/music/royalty-free/licenses/ , https://licenseorg.com/guide/music-audio/incompetech
- Musopen (secondary): https://www.itechguides.com/?p=356055 ; Commons PD-mark copies, for example https://da.wikipedia.com/wiki/Fil:Modest_Mussorgsky_-_night_on_bald_mountain.ogg
- Pixabay Content ID: https://hellothematic.com/thematic-vs-pixabay/
- YouTube Audio Library: https://licenseorg.com/guide/music-audio/youtube-audio-library
- Uppbeat, Bensound: https://licenseorg.com/guide/music-audio/uppbeat , https://licenseorg.com/guide/music-audio/bensound
- CC BY-SA 4.0 legal code (sync clause): https://creativecommons.org/licenses/by-sa/4.0/legalcode.txt

Sound effects
- Freesound: https://freesound.org/docs/api/overview.html , https://freesound.org/docs/api/authentication.html , https://freesound.org/help/tos_api/ , https://freesound.org/help/faq/
- Sonniss: https://sonniss.com/gameaudiogdc
- BBC Sound Effects licence: https://sound-effects.bbcrewind.co.uk/licensing
- OpenGameArt FAQ: https://opengameart.org/content/faq

Stock video and images
- Pexels: https://www.pexels.com/api/documentation/ , https://www.pexels.com/license/ , https://www.pexels.com/terms-of-service/
- Pixabay: https://pixabay.com/api/docs/ , https://pixabay.com/service/license-summary/ , https://pixabay.com/service/terms/
- Unsplash: https://unsplash.com/documentation , https://help.unsplash.com/en/articles/2511245-unsplash-api-guidelines
- Coverr: https://api.coverr.co/docs/ , https://coverr.co/license
- Mixkit: https://mixkit.co/license/ , https://mixkit.co/terms/

Effects, transitions, LUTs, titles
- gl-transitions: https://github.com/gl-transitions/gl-transitions , https://cdn.jsdelivr.net/npm/gl-transitions/gl-transitions.json (measured)
- ISF: https://isf.video/ , https://github.com/Vidvox/ISF-Files (cloned and counted), https://discourse.vidvox.net/t/user-bennoh-is-stealing-shaders-and-uploading-them-online-with-mit-license/2682
- Shadertoy terms: https://www.shadertoy.com/terms (blocked; default licence per https://godotshaders.com/shader/creation-by-silexars/ and others)
- LYGIA licence: https://github.com/patriciogonzalezvivo/lygia/blob/main/LICENSE.md
- Cube LUT specification 1.0: https://kono.phpage.fr/images/a/a1/Adobe-cube-lut-specification-1.0.pdf
- Q-DDL LUTs: https://cgpress.org/archives/800-free-luts.html , https://www.cgchannel.com/2020/01/download-800-free-3d-luts/
- RawTherapee HaldCLUT: https://slackbuilds.org/repository/15.0/graphics/rawtherapee-haldclut/
- G'MIC colour presets: https://gmic.eu/color_presets/
- spektrafilm: https://discuss.pixls.us/t/spektrafilm-luts/57879
- velato: https://github.com/linebender/velato ; dotlottie-rs: https://github.com/LottieFiles/dotlottie-rs ; ThorVG: https://github.com/thorvg/thorvg

Keys and proxy
- Kdenlive online resources: https://invent.kde.org/multimedia/kdenlive/-/tree/master/data/resourceproviders , `src/onlineresources/providermodel.cpp`
- Cloudflare Workers pricing: https://developers.cloudflare.com/workers/platform/pricing/
