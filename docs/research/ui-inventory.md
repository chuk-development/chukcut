# What CapCut's UI is made of

[`capcut-stack.md`](capcut-stack.md) establishes the shape of the desktop app in
one sentence — Qt provides the shell, Chromium renders panels, Lynx runs
JavaScript views — and then moves on. That sentence is true but it is also, as
written, misleading about proportions. This document makes it concrete: which
technology renders which screen, how many screens there are, what they are
called, and where the strings, icons and effect-parameter descriptions that
populate them actually live.

It matters for chukcut because the split turns out to be lopsided in a way that
is directly relevant to our own scoping. The editor proper — timeline, player,
inspector, material browser, export — is **entirely native Qt Quick**, roughly
two thousand QML files compiled into a single DLL. Chromium and Lynx are not
rendering the editor at all; they render the commerce, account, publishing and
web-tool surfaces that sit *around* it. If chukcut only ever builds the editor,
it is competing with the QML half of CapCut and none of the web half. Knowing
the boundary tells us which of CapCut's screens we must match and which are
business scaffolding we can ignore entirely.

Nothing here was decompiled. This is a file survey of a CapCut 9.0.0.3858
installation on Windows (4,032 files, 1.5 GB) plus its user-data directory,
combined with string and resource-table extraction from the shipped binaries and
parsing of the shipped `.pak`, `.po`, `.ini` and SQLite files. No ByteDance
asset — icon, image, font, shader, template or effect package — has been copied
into this repository. Icons and assets are described, never reproduced.

---

## Component versions

The three UI runtimes ship with identifiable versions, which is worth recording
because each one pins a set of capabilities.

| Runtime | Version | Evidence |
|---|---|---|
| Qt | **6.2.2** (vendor build `.586`) | `ProductVersion` on all 27 `Qt6*.dll` |
| CEF / Chromium | **CEF 121.0.0-m121.43, Chromium 121.0.6167.86** | `ProductVersion` on `cef\libcef.dll` |
| Lynx | **3.7.0.12**, engine SDK `3400532428` | `lynx.dll` version; `quickjs_cache\meta.json` |
| App | 9.0.0.3858, commit `d68395d1` | `CapCut.exe`, `VECreator.dll` |

Qt 6.2.2 is an LTS release from late 2021 — the UI toolkit is four years older
than the app around it. `lynx_core_dev.js` carries a build banner dated
`Sat, 14 Mar 2026` and commit `c0348a80`, which matches `lynx.dll`'s
`ProductVersion` exactly, so the shipped JavaScript runtime and the shipped
native runtime were built together.

Notably **absent**: there is no `Qt6WebEngine*.dll` anywhere in the install. Qt's
own browser integration is not used at all; every web surface goes through CEF.

---

## How the install is laid out

The install root is flat — about 240 DLLs and a dozen executables sitting
directly in `%LOCALAPPDATA%\CapCut\Apps\9.0.0.3858\` — with only twelve
subdirectories. Those twelve are where the structure is.

| Directory | Contents | Why it matters |
|---|---|---|
| `cef\` | `libcef.dll` (217 MB), `resources.pak`, `chrome_100_percent.pak`, `chrome_200_percent.pak`, `icudtl.dat`, `snapshot_blob.bin`, `v8_context_snapshot.bin`, `locales\` (55 `.pak`) | The complete, unmodified CEF distribution |
| `QtQuick\`, `QtQml\`, `Qt\`, `Qt5Compat\`, `plugins\` | Stock Qt 6.2.2 QML modules and platform plugins — 354 `.qml` files, 34 `qmldir`, none of them CapCut's | Proves CapCut ships no QML on disk |
| `quickjs_cache\` | One 537 KB bytecode cache plus `meta.json` | Lynx's JS engine is QuickJS; this is its compiled `lynx_core.js` |
| `Resources\` | Effect bundles, fonts, locale `.po` files, H5 web bundles, Lynx template bundles | Everything data-driven |
| `VEAngle\` | `libGLESv2.dll`, `libEGL.dll` | ANGLE for the render engine (covered in `capcut-stack.md`) |
| `Service\` | `CapCutService.exe`, shell-extension DLL and MSIX | Explorer integration, not UI |
| `data\` | A second `icudtl.dat` | ICU for the non-CEF side |

`Resources\` is by far the largest and is mostly not UI: the bulk of it is the
effect-shader corpus (`DefaultAdjustBundle`, `Chroma`, `Curves`, `LogWheel`,
`MixMode`, `figure`, `mattingBlend`, …) described in
[`effect-package-format.md`](effect-package-format.md). The UI-relevant parts of
`Resources\` are five directories, listed later in this document.

### A trap worth naming

`Resources\icon\` sounds like an icon corpus and is not one. It contains
**34 JSON files** named `cc_caption_duration_info_<language>.json` and
`cc_long_caption_duration_info_<language>.json` — seventeen languages, two files
each — holding per-language timing tables for caption display duration. There
are no icons in it. The real icon corpus is not on disk at all; see
[Icons](#icons-are-inside-the-binary) below.

---

## The CEF resource paks contain no CapCut UI

Chromium's `.pak` files are a trivial container: a version word, an encoding
byte, a resource count, then an `(id, offset)` table with the payloads
concatenated after it. Parsing them locally is a twenty-line script, so the
question "how much of CapCut's UI is served out of the paks?" can be answered
exactly rather than guessed at.

The answer is **none**.

| Pak | Size | Resources | Content |
|---|---|---|---|
| `cef\resources.pak` | 8.26 MB | 2,546 (+14 aliases) | Stock Chromium |
| `cef\chrome_100_percent.pak` | 741 KB | — | Stock Chromium UI bitmaps |
| `cef\chrome_200_percent.pak` | 1.1 MB | — | Stock Chromium UI bitmaps (2×) |
| `cef\locales\*.pak` | 454 KB – 1.37 MB each | 9,846 strings in `en-US.pak` | Stock Chromium strings |

Decompressing every gzip-wrapped entry in `resources.pak` yields 16.4 million
characters of text: Polymer-based `chrome://settings` and `chrome://history`
pages, the PDF viewer, a bundled `d3.js`, a bundled `lottie.js`, the TTS
extension manifest, the Password Manager manifest. The strings `capcut`,
`CapCut`, `lveditor`, `bytedance`, `videofusion` and `lynx` appear **zero
times** across the whole blob. `en-US.pak` is the same story: 420 occurrences of
"Chrome", zero of "CapCut".

The conclusion is clean and useful. CapCut did not customise Chromium's resource
bundle at all. Every web surface it renders is either a remote URL or a local
HTML bundle sitting in `Resources\`, loaded through an ordinary `file://` or
`https://` navigation. Nothing is baked into the browser. For chukcut this means
that if we ever embed a web view, we inherit exactly the same options and owe
nothing to a pak-patching build step.

---

## The editor is Qt Quick, and it is all in one DLL

`VECreator.dll` is 236 MB. `capcut-stack.md` lists it as "video engine creator
API", which undersells it: alongside the engine bindings it carries CapCut's
entire QML application as an embedded Qt resource archive.

Extracting UTF-16BE strings from the DLL yields **2,007 distinct `.qml` resource
paths** across **31 QRC modules**, plus about **4,985 asset references** of which
**3,973 are SVG**. For comparison, the entire on-disk `QtQuick\` tree contains
354 `.qml` files, all of them stock Qt controls. CapCut ships no QML, no `.rcc`,
no `.qrc` and no `.qm` file anywhere on disk — it is all inside the binary.

Roughly a third of the QML is stored as plain source rather than compiled
bytecode; 661 files totalling 1 MB of readable QML were recovered, which is how
several of the claims below could be checked directly rather than inferred.

### The QRC module map

| Module | QML files | What it is |
|---|---|---|
| `FunctionPanel` | 463 | The inspector — per-clip property editing |
| `MaterialPanel` | 300 | The material browser — media, audio, text, stickers, effects |
| `GeneralBizUI` | 231 | Shared UI infrastructure: dock widgets, web views, Lynx views, sliders, dialogs |
| `GeneralBiz` | 227 | Shared business UI: feedback, account, personal page, popups |
| `HomeScene` | 198 | The launcher / project browser / cloud & team surfaces |
| `TimeLine` | 121 | The timeline |
| `VipBiz` | 56 | Subscription gating |
| `Player` | 56 | The preview player and its on-canvas controls |
| `Export` | 41 | Export dialog |
| `Agent` | 34 | The AI assistant chat surface |
| `ArticleVideo` | 33 | Article-to-video tool |
| `DigitalHuman` | 28 | Avatar / digital human |
| `SceneManager` | 25 | Main window and scene switching |
| `Purchase` | 24 | Paywall and cashier |
| `Cover` | 23 | Cover / thumbnail designer |
| `TextEditor` | 22 | The transcript-based text editor |
| `WebReview` | 21 | Share-for-review flow |
| `AIModel` | 20 | Model picker |
| `AIShorts` | 17 | Long-video-to-shorts tool |
| `VideoShare` | 16 | Sharing |
| `TemplatePublish` | 14 | Template publishing |
| `AIWriter` | 13 | Copywriting tool |
| `Guide` | 12 | Onboarding overlays |
| `AiTranslate` | 10 | Translation |
| `VEUpdate` | 5 | Updater |
| `MarketingVideo` | 4 | Marketing-video tool |
| `RoughCut` | 2 | Rough cut |
| `Foundation`, `IntelligentCrop`, `TCGlobalSettingsPanel`, `VENetworkDetect` | 1 each | Small entry points |

A separate DLL, `FusionUI.dll` (2.3 MB), holds the design system: 136 QML files
under `/FusionUI/`, split into an `LV*` family (`LVButton`, `LVCheckBox`,
`LVMenu`, `LVLineEdit`, `LVProgress`, `LVTabView`, `LVToolTip`, `LVAlertDialog`,
`LVLoadingDialog`, `LVGradientText`, `LVSVGImage`, …) and a newer `QUI*` family
(`QUIButton`, `QUIColorPanel` with wheel/alpha/brightness/hex/RGBA sub-controls,
`QUICollectionView`, `QUITreeView`, `QUISectionList`, `QUITabControl`,
`QUIFilePanel`). The `LV` prefix is the internal product name — CapCut is
JianYing/LV internally, matching `com.lveditor.draft` in the project format. Two
generations of a component library coexisting in one binary is a familiar smell
and worth remembering when we scope our own.

### The window is a dock layout

Recovered QML shows imports of a `Dockwidgets` module with `frameCpp`,
`DockWidgets.DropLocation_OutterLeft`, `DockWidgets.CursorPosition_TopLeft` and
`frameCpp.redirectMouseEvents(this)`. That API is **KDDockWidgets** (KDAB's
docking framework) in its QtQuick flavour, statically linked and registered from
C++ rather than shipped as a QML plugin directory. It is corroborated on the C++
side: `GeneralBizUI/dock_widget/` contains `DockWidgetRegistry.cpp`,
`DockWidgetTitleBar.cpp`, `DockwidgetLayoutSaver.cpp` and `QQuickItemAdapter.cpp`.

So the editor is not a fixed grid. Every major region is a dock widget hosting a
`QQuickItem`, freely rearrangeable and detachable — confirmed by the string keys
`pc_new_detach_editing_panel` ("Detach editing panel") and
`pc_new_detach_media_panel` ("Detach media panel"). The saved arrangement lands
in `User Data\Config\mainwindowLayoutConfig.json`, a 30 KB base64 blob that
decodes to 22.5 KB of high-entropy bytes with no recoverable strings. It is
either compressed or encrypted; **we could not read it**, so the exact default
dock arrangement is not evidenced here.

### The build tree, as leaked by assert strings

`VECreator.dll` retains 1,496 `__FILE__` paths of the form
`C:\90741\CapCutPC\...`. They give the application's own module decomposition,
which lines up almost exactly with the QRC map and is worth recording because it
names things the QRC paths do not.

```
CapCutPC/
  BizScene/       730 files   EditorScene/{Export,TimeLine,FunctionPanel,MaterialPanel,
                              Agent,Player,WebReview,DigitalHuman,Cover,TextEditor,
                              SmartAssistant,AiTranslate,TemplatePublish,RoughCut,VideoShare}
                              HomeScene/{ViewModels,Draft,NearField,ToolApp,Launchpad,Campaign}
                              ToolScene/{MarketingVideo,ArticleVideo,AIWriter,AIShorts,AIModel}
  Capability/     284 files   Algorithom/{video,audio,aigc,clipflow_nodes,...}
                              Material/{material_insight,importer,tts,panel,clip_flow,...}
                              Editor/{lyra_shell,draft,streaming_edit,video_frame,...}
  GeneralBiz/     175 files   feedback, personal_page, webview_open_helper, user_account_*
  Infra/           80 files   ttnet, http_client, i18n, resource_loader, deep_link, compliance
  GeneralBizUI/    63 files   dock_widget, webview, lynxview, feed_flow, material_data_manager
  GlobalFramework/ 58 files   Main, SceneManager, LaunchTaskScheduler, DeepLinkManager
  Commerce/        57 files   VipGuard, VipBiz, Purchase
  AppMonitor/      30 files   crash and performance monitoring
```

`GeneralBizUI/webview/` (`VEBrowserWidget.cpp`, `webviewitem.cpp`,
`cef_event_monitor.cpp`, `webview_bridge/`) and `GeneralBizUI/lynxview/`
(`lynx_wrapper/`, `resourceloader/`, `lynx_webview_helper.cpp`, `windows/`) sit
side by side under the *shared UI infrastructure* module. Both Chromium and Lynx
are, architecturally, just two more embeddable QML items.

---

## Icons are inside the binary

There is no icon directory on disk. The roughly 4,985 image references extracted
from `VECreator.dll` — 3,973 SVG, 730 PNG, 248 WebP, 30 GIF — are QRC paths into
the embedded resource archive, plus a further 8 SVGs and a handful of PNG/WebP in
`FusionUI.dll`. The naming is systematic enough that the list functions as a
feature inventory on its own.

Each product module owns a resource subtree, and the subtree structure mirrors
the panel structure:

| Module | Assets | Notable subdirectories |
|---|---|---|
| `HomeScene` | 336 | `resources/new_tools`, `resources/new_ui`, `resources/new_homepage_style`, `resources/ToolIcon`, `resources/near_field` |
| `FunctionPanel` | 323 | `resources/Text/Align` (52), `resources/function_assistant` (27), `resources/Text/massEdit` (26), `resources/Video`, `resources/BaseShape`, `resources/Text/AICloneTone`, `resources/Text/AiCloneAvatar`, `resources/AIGC`, `resources/Video/CurvedSpeed` |
| `MaterialPanel` | 244 | `resource/Base` (74), `resource/Media`, `resource/Text`, `resource/Sound`, `resource/ai_generate`, `resource/Caption`, `resource/ai_music`, `resource/Aigc`, `resource/LeftTab` |
| `GeneralBiz` / `GeneralBizUI` | 312 | shared chrome |
| `TimeLine` | 132 | `resources/Canvas`, `resources/ToolBar`, `resources/Canvas/TrackHeader`, `resources/Cursor`, `resources/Drag`, `resources/MaterialReplacement`, `resources/Canvas/AITimelineGenerator` |
| `Agent` | 108 | one flat `resources/` |
| `Player` | 69 | `resource/Image/canvas`, `.../scale`, `.../safe_area`, `.../scope` |

The taxonomy is three-tier: **module → functional area → state-suffixed asset**.
Names describe function, not appearance, and encode state in suffixes: a
checkbox ships as `CheckButtonOn`, `CheckButtonOff`, `CheckButtonOnDisabled`,
`CheckButtonOffGrey`, `CheckButtonOffGreyNew`, `CheckButtonMix`. Brand-kit
variants get a `_brand` suffix (`L-L.svg` / `L-L_brand.svg` in the timeline's
segment-handle set). Retina rasters use `@2x` / `@3x`. Aspect-ratio icons in the
player are literal (`canvas_16_9`, `canvas_9_16`, `canvas_1_1`, `canvas_2p35_1`,
`canvas_1p85_1`, `canvas_5p8`, `canvas_custom`, `canvas_adjust`). Keyboard-key
glyphs are a full set (`A.svg` … `9.svg`, `Alt.svg`, `Backspace_win.svg`,
`BracketLeft.svg`, `Apostrophe.svg`, `Backslash.svg`) because the shortcut
editor draws physical keycaps. Feature entry points in the home screen are WebP
photographs rather than icons (`ai_generate_video_seedance2.webp`, `lip_sync.webp`,
`optical_flow.webp`, `ai_translator.webp`) — a marketing-tile pattern, not an
icon pattern.

The one thing the SVG corpus does *not* cover is animation. `Qt6Bodymovin.dll`
and the `Qt\labs\lottieqt` plugin directory are both present, and `VECreator.dll`
references the `LottieAnimation` type, but only two Lottie JSON assets could be
found (`HomeScene/resources/cloud_not_login.json` and its light-theme twin).
Lottie is shipped and wired up but appears to be used in exactly one place. GIF
(`globalLoading_en.gif` in `FusionUI`) and animated WebP carry most of the
motion instead.

---

## Strings: 16,850 keys of gettext

Localisation lives in `Resources\po\` as **25 GNU gettext `.po` files**, one per
locale, 1.2–2.2 MB each (`en.po`, `zh-Hans.po`, `zh-Hant-TW.po`, `de-DE.po`,
`fr-FR.po`, `ja-JP.po`, `ko-KR.po`, `ru-RU.po`, `th-TH.po`, `vi-VN.po`,
`pt-BR.po`, `es-LA.po`, `id-ID.po`, `ms-MY.po`, `fil-PH.po`, `it-IT.po`,
`pl-PL.po`, `ro-RO.po`, `hu-HU.po`, `cs-CZ.po`, `fi-FI.po`, `nl-NL.po`,
`sv-SE.po`, `tr-TR.po`, `el-GR.po`). `gettext.dll` ships alongside them and
`Infra/Foundation/i18n/ve_i18n_qt.cpp` is the binding. Qt's own `.qm` mechanism
is not used at all — there is no `.qm` file in the install.

`en.po` holds **16,850 `msgid` entries** (16,544 non-empty). Crucially the
`msgid`s are **keys, not English source strings** — the header names the
translation platform (`Report-Msgid-Bugs-To: starling`, ByteDance's internal
i18n service), and recovered QML calls them as `i18n.tr("pc_failed_to_load")`.
So the key list is a direct, machine-readable map of the product surface.

Key namespaces, by first token:

| Prefix | Keys | Meaning |
|---|---|---|
| `pc_` | 11,628 | Desktop client (the bulk) |
| `cc_`, `ccug_`, `ccpc_` | 730 | CapCut-brand-specific and growth strings |
| `ai_`, `shorts_`, `voice_`, `avatar_` | 900 | AI feature families |
| `mob_`, `mobile_`, `pad_` | 155 | Shared with mobile/iPad builds |
| bare English-ish keys | ~2,000 | Legacy keys named after their English text |

Within `pc_`, the second token is effectively a feature index. The largest
namespaces:

```
pc_voice 586   pc_ai 492      pc_m10n 409    pc_text 375    pc_avatar 258
pc_cloud 193   pc_scene 184   pc_template 173 pc_recommend 170 pc_charts 164
pc_clipper 150 pc_material 147 pc_translate 143 pc_subtitle 139 pc_stickers 131
pc_effect 126  pc_overdub 117 pc_caption 104 pc_library 104  pc_aigc 98
pc_export 93   pc_color 92    pc_materiallib 87 pc_language 85 pc_brand 78
pc_edit 77     pc_home 77     pc_lipsync 72  pc_retouch 64  pc_sticker 61
pc_shape 60    pc_inpainting 59 pc_photoacting 58 pc_recorder 57 pc_light 54
pc_outpainting 49 pc_aimusic 48 pc_fashionmodel 46 pc_teams 45 pc_filter 43
pc_track 43    pc_resize 39   pc_tts 37      pc_backup 34   pc_trash 30
```

`pc_m10n` is "monetisation" (m + 10 letters + n) at 409 keys — the fourth-largest
namespace in the product, ahead of text.

There is a second, subtler signal in the key naming that turns out to predict the
rendering technology. Older native panels use flat snake_case
(`pc_materiallib_audio_mine_favorite_music`,
`pc_maintrack_linkage_panel_cliptype_sticker`). Newer surfaces use a
component-path convention with camelCase segments and typed leaf tokens —
`pc_library_assetLibrary_navBar_tab_generatedAssets`,
`pc_library_assetDeleteModal_btn_cancel`,
`pc_library_assetDetail_actionSection_btn_download`. That `_navBar_`, `_btn_`,
`_Modal_`, `_Section_` grammar is a React/Lynx codebase convention, not a QML
one, and the surfaces it appears on are the ones we independently classify as
web or Lynx below.

A separate, smaller string file exists at `User Data\Config\effectParamNames.json`
— 21 entries mapping effect-parameter keys to localised labels
(`effects_adjust_intensity` → "Stärke", `effects_adjust_speed` → "Geschw.").
This is a **server-populated cache**, not a shipped resource; its role is
explained in the effect-parameter section below.

---

## The Lynx surfaces

Lynx is ByteDance's React-Native equivalent. On this install it renders
**fourteen channels** and is used for exactly one thing: commerce and account
modals.

`Resources\lynx_config\lynx_config.json` is the routing table — 22 named routes
mapping to `videofusion://main/lynx?channel=…&bundle=…&window_width=…&window_height=…`
URLs. The window dimensions are in the URL, which tells you what these are: they
are all small fixed-size popups, none of them larger than 512×718, most of them
around 356×486.

| Route | Channel | Window | Purpose |
|---|---|---|---|
| `credits_center`, `credits_center_ng`, `credits_history` | `..._credits_center*`, `..._credits_history_ng` | 512×707 | AI credit balance and history |
| `image_lynx_videocutpc_subscription` | `..._subscription` | 480×648 (800×648 en-US) | Subscription plans |
| `commerce_export_modal` | `..._subscription_export` | 480×600 | Paywall on export |
| `..._subscription_manage` | `..._subscription_manage` | 356×590 | Manage subscription |
| `commerce_aigc_purchase` | `..._subscription_pay_modal` / `pages/aigc` | 352×220 | Buy AI credits |
| `digital_human_purchase` / `_activate` | `.../pages/digital_human` | 512×718 / 352×352 | Avatar purchase |
| `remove_watermark` | `.../pages/remove_watermark` | — | Watermark removal upsell |
| `..._subscription_teams` / `_cashier` | `..._subscription_teams` | — | Team plans and checkout |
| `trade_record` | `..._subscription_trade_record` | 356×593 | Purchase history |
| `message_center`, `feedback_detail` | `..._message_center` | — | Notifications, feedback thread |
| `image_lynx_videocutpc_restore_page` | `..._restore_page` | 356×522 | Restore purchases |
| `image_lynx_videocutpc_modal`, `commerce_export_tips` | `..._modal` / `commerce_splash_modal`, `editor_vip_benefit_popup` | 356×486 | Promo splash, Pro-benefit popup |
| `global_home` | `..._global` | 338×203 | Home-screen popup |
| `vicut_lynx_commerce_comp_popup` | `vicut_lynx_commerce_comp_popup` | 338×166 | Compensation popup |

Each channel ships one or more `template.js` files — 17 in total, 64 KB to
2.3 MB — which are **not** JavaScript despite the extension. They are binary Lynx
template bundles: a header (`0.2.0.0`, target SDK `2.5`), then tagged sections
(`OFNI` = INFO, `JSBI`, `XMLH`, `CSSI`) carrying the element tree, the style
sheet and an embedded `/app-service.js`. The embedded JS is wrapped in Lynx's
standard sandbox signature, which enumerates the entire global environment a
Lynx card is given:

```js
tt.define("app-service.js", function(e, r, t, n, setTimeout, setInterval,
  clearInterval, clearTimeout, NativeModules, tt, console, a, o, nativeAppId,
  Behavior, LynxJSBI, lynx, window, document, frames, self, location, navigator,
  localStorage, history, Caches, screen, alert, confirm, prompt, fetch,
  XMLHttpRequest, WebSocket, webkit, Reporter, print, global) { … }
```

### The Lynx native bridge

Every one of the fourteen bundles talks to the host through a single module,
`NativeModules.bridge`, invoked as `.call("<method>", args, callback)`. Across
all bundles the complete method surface is **35 calls**:

```
assetLocate           closeView              copyToClipboard      fetch
getAppInfo            getCreditsConfig       getEntrancesConfig   getNativeStorageItem
getPipoContextAndRiskInfo                    getPriceStrategy     getSettings
getUserInfo           getVipInfo             getWorkspaceInfos    iapPurchase
login                 open                   openGlobalPopUpDialog
openPicture           postDeepLink           preFetchIAPProduct   reportUsage
restoreIAP            sendEvent              sendLog              setFeedbackReadedMessageId
setNativeStorageItem  setTitle               setWindowSize        showDialog
signFetch             startIAP               toast                videoExport
```

Read that list as a specification of what Lynx is *allowed* to do, and the
classification becomes unarguable. There is no timeline call, no material call,
no draft call, no render call. It is purchase, identity, telemetry, storage,
window chrome and a dialog. Lynx cannot touch the editor.

The runtime surface confirms it from the other side: `lynx_core_dev.js` exposes
`lynx.requireModule`, `lynx.getJSContext`, `lynx.__globalProps`,
`lynx.performance`, `lynx.fetch`, `getJSModule("GlobalEventEmitter")` and
`getJSModule("BDLynxAPIModule")`, with lifecycle intrinsics `__Card__`,
`__OnNativeAppReady`, `__OnAppFirstScreen`, `__OnReactCardRender` and session
storage. Named `NativeModules` beyond `bridge` are just `LynxFetchModule` and
`LynxRecorderReplayDataModule`. Element tags in the bundles are Lynx's standard
primitive set — `view`, `text`, `image`, `list`, `swiper`, `raw-text` — with an
`x-tt-env` marker. There is no rich component library here; these are simple
cards.

The C++ side (`GeneralBizUI/lynxview/lynx_wrapper/`) exposes
`lynx::LynxViewBase` with `LoadTemplate`, `SendGlobalEvent`, `SetGlobalPropsData`,
`UpdateScreenMetrics`, `SetLayoutWidthMode`/`HeightMode` and a `LynxViewClient`
callback interface — a standard embedder integration. The QML side wraps it as a
`LynxWebViewItem`, and recovered source shows exactly how a Lynx surface is
presented:

```qml
LVModalDialog {
    property LynxPopupViewModel viewModel: LynxPopupViewModel{}
    fixedHeight: viewModel.height
    fixedWidth: viewModel.width
    LVWindow.TitleBar { id: titleBar }
    LynxWebViewItem {
        id: webItem
        sourceUrl: viewModel.channel()
        LoadTipPanel {
            failLoadingText: i18n.tr("pc_failed_to_load")
            isLoading: WebViewItem.Loading === webItem.loadingState
            onSignalRetry: webItem.reload()
        }
    }
}
```

A Qt window, a Qt title bar, a Qt loading/retry panel, with a Lynx rectangle in
the middle. The `i18n.tr("pc_failed_to_load")` retry state is itself telling:
these surfaces can fail to load, and the native shell owns the failure UI.

---

## The CEF surfaces

Chromium renders three distinct categories of surface, all of them outside the
editing loop.

**1. Locally-bundled H5 apps in `Resources\`.** Five directories hold complete
static web builds — HTML entry points plus hashed `static/js` and `static/css`
chunks. The route manifests name the framework: `nestedRoutes.json` entries
reference `@_edenx_src/pages/<page>/routes/{layout,page}`, and one channel ships
a `modern.config.json`. That is **EdenX**, ByteDance's Modern.js-based React
framework. The chunk names (`lib-react`, `lib-mobx`, `lib-router`,
`lib-polyfill`, `lib-arco`) name the stack: React + MobX + Arco Design.

| Directory | Entry points | Surface |
|---|---|---|
| `image_h5_material_publish` | 8 HTML: `publish-video`, `publish-effects`, `publish-filter-single`, `publish-filter-collection`, `publish-audio-effect`, `publish-caption-animation`, `publish-caption-template`, `publish-text-animation` | Creator publishing flows |
| `image_h5_sticker_publish` | `sticker-publish-single`, `sticker-publish-collection`, `creator-protocol-dialog` | Sticker publishing |
| `image_h5_text_effect_publish` | `publish-text-effect` | Text-effect publishing |
| `image_h5_text_template_publish` | `publish-text-template`, `publish-text-template-collection`, `publish-subtitle-template` | Text-template publishing |
| `lvop_intelligent_crop` | `index.html`, `<title>SmartCut</title>` | The SmartCut / intelligent reframe tool |

The `publish-effects.html` head is a plain SSR-capable EdenX shell with
`<body theme-mode="dark">`; SmartCut's is `<body arco-theme="dark">` and opens by
overriding `document.cookie` to throw on write.

**2. Remotely-hosted pages.** `VECreator.dll` contains 33 distinct
`https://www.capcut.com/...` URLs. Stripping legal and OAuth boilerplate, the
ones that are product surfaces are:

| URL | Surface |
|---|---|
| `/editor-tools/long-video-to-shorts` | AI Shorts (`AIShorts` QRC module) |
| `/editor-tools/marketing-video-pc` | Marketing Video tool |
| `/editor-tools/smart-text-editor-pc` | Smart text editor |
| `/editor-tools/refer-image-editor` | Reference-image editor |
| `/editor-graphic`, `/ai-design-cover-pc` | Design Studio / AI cover design |
| `/magic-tools/text-to-speech` | TTS |
| `/ai-creator/start/`, `/ai-creator/ideas/start` | AI creator onboarding |
| `/lvpc_web/{login,login_proxy,login_status,login_success,questionnaire,share_oauth_success,tt_account_bind_guide}` | Account and sharing |
| `/capcut_pc_web/fission_receive` | Growth / referral |

The CEF disk cache corroborates which of these actually ran on this machine. It
holds bundles from five deployment roots on `capcutstatic.com`:
`ies/capcut_pc_web_os` (login and upgrade popups), `ies/ccweb/ai_design_cover_online`
(AI Design Cover), `ies/capcut_web_ug` (growth/fission pages, TikTok Sans fonts),
`ies/lvweb`, and `ies/pippit` (a landing page). All are React/Modern.js builds
with `lib-arco`.

**3. Gecko-delivered channels.** `TTPGeckoCppSDK.dll` is ByteDance's dynamic
resource-delivery client. `User Data\Cache\GeckoCpp\resources\<hash>\` holds
**20 channels** — the 14 Lynx channels above plus 6 H5 ones, each versioned by a
numeric build id. The H5 set is the four publish apps already shipped on disk
(so Gecko is updating them out of band), plus two that are **not** in the install:
`image_h5_smart_edit_chat_bot` (`chat-bot.html`) and `image_h5_user`
(`main.html`, `upload-sticker.html`). This is the mechanism by which CapCut ships
UI without shipping an update, and it applies to both web and Lynx equally.

### The CEF native bridge

The web bridge is symmetric with the Lynx one but simpler. `VECreator.dll`
contains the injection templates:

```
window.JSBridge._handleCallbackFromNative(`%1`)
window.JSBridge._handleEventFromNative("%1")
window.%1JSBridge._handleCallbackFromNative(`%2`)
```

with the implementing class `lvop::JSBridge` and, on the embedding side,
`platinum::WebView` (`PlatinumWebView.dll`, a thin C wrapper over CEF exposing
`createBrowser`, `excuteJS` — sic — `goBack`, `beforeNavigation`,
`beforeDownload`, `dragFilesUpdated`, `TabIdUpdated`). `CefCreator.dll` is the
subprocess helper and, notably, also handles `pc_load_lynx_url` — the same host
process serves both runtimes.

The QML types are `WebViewItem` (with a `loadingState` enum:
`Loading` / `LoadEnd`) and `LynxWebViewItem`, and both are consumed the same
way: `viewModel.setWebViewItem(webItem)`.

---

## The UI technology split, with evidence

This is the section the rest of the document exists to support.

### Native Qt Quick

**Everything in the editing loop, plus the home screen, plus every dialog and
window frame.**

Evidence:

1. `VECreator.dll` embeds 2,007 QML files whose QRC prefixes are `TimeLine`,
   `Player`, `FunctionPanel`, `MaterialPanel`, `Export`, `Cover`, `TextEditor`,
   `HomeScene`. These are not shell wrappers — `FunctionPanel` alone has 463
   files including `DraftInspectorVideo.qml`, `DraftInspectorAudio.qml`,
   `DraftInspectorText.qml`, `DraftInspectorAdjust.qml`,
   `DraftInspectorEffect.qml`, `DraftInspectorFilter.qml`,
   `DraftInspectorTransition.qml`, `DraftInspectorShape.qml`,
   `DraftInspectorSticker.qml`, `DraftInspectorPluginEffect.qml`,
   `DraftInspectorMultipleSelection.qml` and `DraftInspectorProjectConfig.qml`.
2. `TimeLine` contains `MainTimeLine.qml`, `MainTimeLineCanvas.qml`,
   `TimelineFlickable.qml`, `MainTimelineDocker.qml`, and a
   `MainTimeLineSegment*` file for every clip type (Video, Audio, Text,
   CaptionText, TextTemplate, Sticker, ImageSticker, Shape, Effect, Filter,
   Adjust, Transition, PluginEffect, Handwrite). A web view does not need
   per-clip-type QML files.
3. `Player` contains `PlayerSeekBar.qml`, `PlayerToolbar.qml`,
   `PlayerInteractiveArea.qml`, `GenericMaskEditControl.qml`,
   `ManualFaceBox.qml`, `MultiCameraPanel.qml`, `VideoScopesPanel.qml`,
   plus `view/tracking_control`, `view/safe_area`, `view/reference_line`,
   `view/player_ruler` and `view/area_select`. On-canvas manipulation is native.
4. No CapCut QML, `.rcc`, `.qrc` or `.qm` file exists on disk; the only on-disk
   QML is the stock Qt 6.2.2 module tree.
5. The design system (`FusionUI.dll`, 136 QML components) exists at all, which
   would be pointless if panels were HTML.
6. The build tree confirms it: `BizScene/EditorScene/{TimeLine,FunctionPanel,
   MaterialPanel,Player,Export,Cover,TextEditor}` is 400+ C++ files of view-model
   code.

### CEF / Chromium

**Publishing, account, sharing, feedback, and the standalone AI web tools.**

Evidence:

1. QML files whose names announce the embedding:
   `GeneralBiz/webview_open_helper/{RouterWebView,ModalRouterWebView,
   NormalRouterWebView,PopupRouterWebView}.qml`,
   `GeneralBizUI/web_dialog/VEWebDialog.qml`,
   `Cover/CoverViews/CoverEditorWebView.qml`,
   `AIShorts/views/AIShortsWebViewWindow.qml`,
   `MaterialPanel/text/ai_write/web/AIWriteWebWindow.qml`,
   `HomeScene/Launchpad/Views/HomePageWebViewX.qml`,
   `GeneralBiz/feedback/FeedbackViews/Feedback{Custom,IM,SmartIM}WebItem.qml`,
   `GeneralBiz/personal_page/webpage/WebPersonalPageDialog.qml`,
   `GeneralBiz/user_account_cancellation/.../UserAccountCancellationWebDialog.qml`,
   `GeneralBiz/user_pick_role/Views/UserPickRoleWebView.qml`, and the 21-file
   `WebReview` module.
2. Five complete EdenX/React app bundles shipped in `Resources\`, all of them
   publishing flows or SmartCut.
3. Nine product URLs on `www.capcut.com` referenced from the binary, matching
   QRC modules one-to-one (`/editor-tools/long-video-to-shorts` ↔ `AIShorts`,
   `/editor-tools/marketing-video-pc` ↔ `MarketingVideo`, `/ai-design-cover-pc` ↔
   `Cover`).
4. The CEF disk cache contains actual downloaded React bundles from five
   `capcutstatic.com` deployment roots.
5. `PlatinumWebView.dll` exists as a dedicated CEF wrapper, and
   `GeneralBizUI/webview/webview_bridge/` implements `window.JSBridge`.

**Counter-evidence for the editor being web:** the paks contain zero CapCut
content, and the CEF cache contains no editor-shaped bundle — only login,
upgrade, cover design and growth pages.

### Lynx

**Commerce, credits, subscription, notifications. Fourteen channels, all popups.**

Evidence:

1. `Resources\lynx_config\lynx_config.json` enumerates 22 routes across 14
   channels, every one of them subscription, credits, purchase, restore, trade
   record, message centre or promo popup, each with an explicit fixed window size
   of at most 512×718.
2. The `NativeModules.bridge` method surface across all 17 shipped bundles is 35
   calls, none of which touch the timeline, the draft or the renderer.
3. The QML hosts are `GeneralBizUI/lynxview/lynx_wrapper/LynxDialog/LynxDialog.qml`,
   `GeneralBiz/common_popup_manager/LynxPopupWindow/LynxPopupWindow.qml`,
   `GeneralBiz/customise_popup/views/LynxPopupView.qml`,
   `HomeScene/Views/purchase/LynxVipCard.qml` and
   `HomeScene/Campaign/campaign/HomePageLynxCampaign.qml` — all dialogs, popups
   and cards.
4. `Commerce/VipBiz/lynx_vip_rights_controller.cpp` places the Lynx controller
   inside the commerce module, not the editor module.

### Summary table

| Surface | Technology | Confidence |
|---|---|---|
| Timeline, tracks, segments, keyframes | Qt Quick | Certain |
| Preview player, on-canvas masks/tracking/scopes | Qt Quick | Certain |
| Inspector (all clip types) | Qt Quick | Certain |
| Material browser (media, audio, text, stickers, effects, filters, transitions) | Qt Quick | Certain |
| Export dialog | Qt Quick | Certain |
| Home screen, drafts, cloud, teams, brand kit | Qt Quick | Certain |
| Text/transcript editor | Qt Quick | Certain |
| Menus, dialogs, window chrome, shortcuts editor | Qt Quick | Certain |
| AI assistant chat (`Agent`) | Qt Quick shell; content unverified | Likely native |
| Cover designer | Qt Quick shell + CEF (`CoverEditorWebView.qml`, `/ai-design-cover-pc`) | Certain |
| AI Shorts, Marketing Video, Smart Text Editor, AI Write | CEF, remote | Certain |
| SmartCut / intelligent crop | CEF, local bundle | Certain |
| Creator publishing (video, effects, filters, stickers, text templates) | CEF, local bundle | Certain |
| Web review / share-for-review | CEF | Certain |
| Login, account, personal page, account deletion, feedback IM | CEF | Certain |
| Subscription, paywall, cashier, credits, trade record | Lynx | Certain |
| Message centre, feedback detail | Lynx | Certain |
| Promo splash, VIP benefit popup, restore purchases | Lynx | Certain |

---

## Panels and screens we can evidence

Enumerated from QRC paths, string keys and cached panel metadata. This is not a
guess at CapCut's feature list; every entry below has a named QML file, a
localisation key namespace or a server panel record behind it.

### Editor — inspector (`FunctionPanel`, 463 QML)

Sub-panels exist for `video`, `audio`, `text`, `adjust`, `effect`, `filter`,
`transition`, `sticker`, `shape`, `mask`, `plugin_effect`, `multiple_selection`,
`information`, `project_config`, `rough_cut`, `sub_draft`, `task_center`,
`template`, `template_creator`, `common` and `container`. The heaviest
sub-trees are `video/screen` (49 files — the basic transform/blend/background
tab), `template_creator/Text` (46), `text/Edit` and `text/EditEx` (45 combined),
`project_config/function_assistant` (26), `video/aigc` (24),
`video/digital_human` (13), `audio/effect` (13), `text/ai_clone_tone` (12),
`shape/ColorPanel` (8), `video/speed` (7), `mask/text_mask` (6). Colour work has
dedicated files: `AdjustColorCurveView.qml`, `AdjustColorWheelView.qml`,
`AdjustHslSettingView.qml`, `ColorWheelComponentEx.qml`,
`AdjustPresetSaveView.qml`, `adjust/color_match`.

The container is `FunctionPanel/core/InspectorConatiner.qml` — the typo is
theirs, and is a small confirmation that these are hand-written source files
rather than generated ones.

### Editor — material browser (`MaterialPanel`, 300 QML)

Two generations coexist. The current one lives under `new_panel/` and has one
inspector per tab: `NewMediaInspector`, `NewSoundInspector`, `NewCaptionInspector`,
`NewFilterInspector`, `NewAdjustInspector`, `EffectInspector`,
`NewAiAvatarInspector`, `NewAiPackageInspector`. Tab names come from the
`pc_materiallib_*` key namespace: **Media**, **Audio** (Music / Sound effects /
TikTok link import / Brand music / Favourites), **Captions** (auto, manual entry,
SRT/LRC/ASS import), **Text**, **Stickers**, **Effects** (video / body),
**Transitions**, **Filters** (incl. LUT import), **Adjustment** (incl. brand
presets and personal presets), **Templates**.

Supporting views name the integrations: `giphy/GiphyPanel.qml`,
`media/dreamina/DreaminaPanelView.qml`, `sound/tiktok_mark/*`,
`sound/NewOnlineTiktokMarkMusicLibrary.qml`, `media/qr_upload` (phone-to-desktop
import, matching the `pc_scanupload_codepanel_*` keys), `media/search`,
`text/ai_write`, `text/ai_lyrics_effect`, `text/ai_packaging`,
`music/ai_music`, `music/ai_sound`, `media/ai_generate`,
`media/ai_image_interpretation`, `aigc_text_template/*`.

### Editor — timeline (`TimeLine`, 121 QML)

`MainTimeLine`, `MainTimeLineCanvas`, `MainMultiTimelineLayout`,
`MainTimelineDraggableListView`, `MainTimelineTabItem` (multiple timelines in
tabs), `MiniTemplateTimeLine`, `CreateMultiCam`. Per-type segment renderers for
15 clip types. Dedicated subsystems: `closegap/` (ripple delete),
`dragfollow/`, `linkage/` (the "linkage settings" panel keyed by
`pc_maintrack_linkage_panel_*`), `rangeselect/`, `replacement/`,
`segment/keyframe/`, `segment/speedcontrol/`, `track/header/`,
`ai_timeline_generator/view/`, and three menus (`TimeLineMenu`, `BasicEditMenu`,
`PrerenderMenu`).

### Editor — player (`Player`, 56 QML)

`Player`, `PlayerSeekBar`, `PlayerToolbar`, `PlayerInteractiveArea`,
`PlayerEffectControlArea`, `FullScreenToolBar`, `RatioSelectMenu`,
`VideoScopesPanel`, `ExportSingleFrameDialog`, `MultiCameraPanel` /
`MultiCameraPage`, `GenericMaskEditControl`, `ManualBeautyBodyControl`,
`ManualFaceBox`, `RecommendBubble`, plus `view/tracking_control` (7 files),
`view/custom_tracking`, `view/safe_area`, `view/reference_line`,
`view/player_ruler`, `view/area_select`, `view/audio_indicator`. Alternative
players exist for each tool mode: `RoughCutPlayer`, `AiShortsPlayer`,
`AiTranslatePlayer`, `SceneTemplatePlayer`, `CutSameTemplatePreviewPlayer`,
`TemplatePublishPlayer`, `AETemplatePublishPlayer`, `text_rough_editor_player`.

### Home (`HomeScene`, 198 QML)

`HomePage` with tab control, local drafts, cloud drafts, open-project list,
ratio selection, tool panels (`HomePageToolPanel`, `HomePageNewToolPanel`,
`HomePageNewTopToolPanel`), plus subtrees for `enterprise` (32),
`team` (22), `cloudentry` (18), `brandkit` (13), `Launchpad/Views` (12),
`webreview` (9), `drafts` (8), `nearfield` (5, phone-to-desktop),
`highlightPenetrate` (4) and `campaign` (4). The navigation grouping is named by
keys: `pc_home_nav_menu_video_studio`, `..._design_studio`, `..._library`,
`..._create_with_ai_header`, `..._mgmt_header`, `..._more_tools`.

### Export (`Export`, 41 QML)

33 files under `business/panel`, plus `business/aicrop`,
`business/enterprise` and `business/backend_export` (background/queued export,
matching `pc_taskmanage_exportpanel_*` and `pc_queue_*` keys).

### Cover designer (`Cover`, 23 QML)

`CoverSelectorView`, `CoverEditorPanel`, `CoverEditorView`, `CoverDesignerView`,
`CoverTextEditPanel`, `CoverMaterialPanel`, `CoverPlayerPreview`, plus a
`CoverEditorWebView` — and a `*Legacy` twin for the selector and designer.

### Text / transcript editor (`TextEditor`, 22 QML)

`TextEditorView`, `TextParagraphView`, `TextRoughEditorWindow`, six files under
`Views/TranscriptEdit`, three under `TextEditOverdub/Record`, and four text
template info views.

### Tools and AI (separate QRC modules)

`Agent` (34 — the AI assistant), `ArticleVideo` (33), `DigitalHuman` (28),
`AIModel` (20), `AIShorts` (17), `AIWriter` (13), `AiTranslate` (10),
`MarketingVideo` (4), `RoughCut` (2), `IntelligentCrop` (1).

### Commerce (`VipBiz` 56 + `Purchase` 24 QML, plus 14 Lynx channels)

Paywall, cashier, cloud-space purchase, auto-renewal, rights checking,
limit-benefit, cold-start promo — the QML half; the Lynx half is the actual
purchase and credits UI.

### Keyboard shortcuts

`User Data\Config\Shortcut\` holds five keymap files: `Custom1/2/3.json`,
`Final Cut Pro X.json` and `Premiere Pro.json`. Each maps **86 named actions** to
key sequences. The action names are a compact list of what the editor can do
without a mouse: `cutoff`, `cutLeft`, `cutRight`, `batchCut`, `adsorb`,
`mainTrackAdsorb`, `linkage`, `addKeyframe`, `addBasicKeyframe`,
`expandKeyframePanel`, `divideSpeedSegment`, `activeSpeedControl`,
`segmentCombination`, `segmentMakeGroup`, `prerenderSelectArea`,
`selectRangeStart`/`End`/`BySegment`, `markBeat`, `markWithAnotherColor`,
`nextCutPoint`, `storeSingelFrame` (sic), `toggleSegmentVisibe` (sic),
`customMattingSwitchAiEraser`, `subtitleSplit`, `subtitleNewLine`,
`copySegAttribute`/`pasteSegAttribute`, `playerZoomFit`/`In`/`Out`,
`speedPlayForward`/`Backward`, `refreshCloudDraft`, `videoRecord`.

---

## How effect parameters become sliders

This is the pipeline chukcut most directly needs to replicate, and it is fully
observable without decompiling anything.

### 1. The effect package carries no UI description

Unpacking downloaded effect packages from `User Data\Cache\effect\<id>\<md5>\`
shows `config.json` (link table and version), `amazingfeature/` with
`content.json`, `main.scene`, `.prefab`, `.material`, `.mesh`, `.rt`, `.lua` and
`.xshader` files — the render graph and nothing else. The one file whose name
suggests a UI descriptor, `ImageBusinessSlider.json`, is present but reads
`{"ImageBusinessSlider": null}` in the shipped copies we checked. **The package
does not describe its own controls.**

### 2. The parameter description comes from the effect platform, over HTTP

`User Data\Cache\ressdk_db\<uid-hash>\rp.db` is a SQLite database (up to 133 MB)
with the resource-panel schema: `panel_info(name, version, panel_source, …)`,
`panel_category(panel, category_id, category_key, category_index)`,
`category_data(category_id, category_name, category_key, category_icon,
category_selected_icon, sub_categories, category_extra, …)`,
`category_effect(category_id, effect_id, effect_index, …)` and a very wide
`effect` table (180+ columns) plus a parallel `loki_*` set for the second panel
backend. On this install the normalised tables were empty and the data sat in an
`http_cache(url, response_body, version, timestamp)` table — 340 cached responses
to `/artist/v1/panel/get_panel_info_<hash>_<panel>_capcutpc_general` and
`/artist/v1/effect/get_resources_by_category_id_<hash>_<panel>_capcutpc_general`.

Panel names visible in those URLs on this machine: `video`, `voice-change`,
`shuziren2408` (digital human), `gameplay`, `ai_painting`, `subtitle-templates`,
`velocity`, `auto-beauty2`, `Auto_hair`, `complex_video_mask`, `aiavatarmask`,
`ai_character_background`, `default`. `panel_source` is `heycan` for 71 of them
and `loki` for one — two generations of effect backend, matching the two table
families.

Inside each effect record, the field that drives the UI is `sdk_extra`, a
JSON-in-JSON string. Its top-level keys across the cache are `setting`
(6,060 occurrences), `depend_resource_list`, `script_template_version`,
`isEHEffect`, `triggerJson`, `caption_setting`, `External_Producer`,
`transition`. Inside `setting`, the key that matters is **`effect_adjust_params`**:

```json
{
  "algorithm": "velocity_edit@flash",
  "is_local": true,
  "is_async": false,
  "ability_flag": 4,
  "adjustable_config": { "enable_preview": true },
  "effect_adjust_params": [
    { "effect_key": "effects_adjust_intensity", "default": 0.5, "min": 0, "max": 1 }
  ]
}
```

Each array entry is exactly one slider: a **key**, a **default**, a **min** and a
**max**. Nothing else. There is no widget type, no step, no curve, no grouping —
the client renders every one of them as the same control.

The complete `effect_key` vocabulary observed in the cache is 22 values:

| Key | Uses | Key | Uses |
|---|---|---|---|
| `effects_adjust_speed` | 159 | `effects_adjust_horizontal_shift` | 14 |
| `effects_adjust_intensity` | 81 | `effects_adjust_rotate` | 13 |
| `effects_adjust_size` | 46 | `effects_adjust_vertical_shift` | 11 |
| `effects_adjust_luminance` | 37 | `effects_adjust_sharpen` | 11 |
| `effects_adjust_background_animation` | 35 | `effects_adjust_horizontal_chromatic` | 8 |
| `effects_adjust_blur` | 34 | `effects_adjust_distance` | 6 |
| `effects_adjust_filter` | 32 | `effects_adjust_alpha` | 6 |
| `effects_adjust_color` | 25 | `effects_adjust_number` | 6 |
| `effects_adjust_range` | 22 | `effects_adjust_noise` | 5 |
| `effects_adjust_distortion` | 18 | `effects_adjust_vertical_chromatic` | 3 |
| `effects_adjust_texture` | 17 | `effects_adjust_soft` | 1 |

A twenty-two-word vocabulary covering every third-party effect in the catalogue.
That is a deliberate design choice and a cheap one to copy.

### 3. The label comes from a separate, server-populated cache

`effect_key` is not localised in the response. The client keeps
`User Data\Config\effectParamNames.json` — the same 21 keys mapped to
locale-appropriate labels (`effects_adjust_intensity` → "Stärke",
`effects_adjust_speed` → "Geschw.", `effects_adjust_texture` → "Struktur"). The
same keys appear as string literals in `VECreator.dll`, so the client has
fallbacks, and a sibling file `effectParamNames_loki.json` exists for the other
backend. The label table is therefore **remotely extensible**: a new
`effect_key` can be introduced server-side and named without shipping a client.

### 4. Local panels are pre-baked in the same shape

`User Data\Config\Modules\` holds `.ini` files carrying Qt `QVariant`-serialised
panel descriptors, cached so that built-in panels open without a network round
trip. `beauty_panels_en.ini` is 786 KB and holds sections `auto-beauty`,
`auto-beauty2`, `auto-beauty3`, `makeup`, `makeup_root`, `manual-figure`,
`manual_beauty`, `skinColor`, `face_box`, with per-item fields `category_id`,
`name`, `defaultValue`, `minValue`, `maxValue`, `effect_id`, `effect_key`,
`effect_type`, `exclusion_group`, `face_detect`, `icon_uri`, `icon_url`,
`is_prefab`, `order`, `ratio`, `resource_id`, `sub_type`, `vip_name`,
`composer_name`. Sibling files use a slightly older, flatter shape
(`defaultValue`, `effectID`, `resourceID`, `isVip`, `minValue`, `maxValue`,
`md5`, `name`, `path`): `adjust_collection.ini` (colour adjustment),
`complex_video_mask.ini` (Circle, Rectangle, Heart, Star, Filmstrip, Brush, Pen,
Split, Text masks), `camera-movement.ini` (SmartMotion I/II),
`camera_tracking.ini`, `matting.ini`.

Note `exclusion_group` and `face_detect` in the beauty descriptor — the only two
pieces of control *semantics* found anywhere in the pipeline: mutual exclusivity
between items, and a precondition that a face be detected.

### 5. The chosen value goes back into the draft

The value a slider produces is written into `draft_content.json` as
`video_effects[].adjust_params: {"name": "<effect_key>", "value": <float>}` — a
schema sample of exactly this shape is embedded in `VECreator.dll` (it belongs to
the deep-link scripting API). The engine side is
`lvve::EffectAdjustParamsInfo`, referenced from `MaterialEffect`,
`MaterialVideoEffect`, `Gameplay`, `FaceAdjustParamsInfo` and `MattingStroke`,
which matches the material categories catalogued in
[`draft-format.md`](draft-format.md).

### The pipeline in one line

> effect package (render graph only) → server `sdk_extra.setting.effect_adjust_params`
> `[{effect_key, default, min, max}]` → generic slider, labelled by
> `effectParamNames.json` → `draft.materials.video_effects[].adjust_params
> {name, value}` → `lvve::EffectAdjustParamsInfo`

For chukcut the implication is that a *single* generic parameter control, driven
by a four-field descriptor and a key→label table, is enough to render every
effect panel CapCut has. Complexity lives in the shader graph, not in the UI.

---

## What we could not access

Stated plainly rather than guessed at:

- **`mainwindowLayoutConfig.json`** decodes from base64 to 22.5 KB of
  high-entropy bytes with no extractable strings. The default and current dock
  arrangement of the main window is therefore not evidenced here. We know the
  window *is* a KDDockWidgets layout; we do not know its default geometry.
- **Compiled QML.** Only 661 of the 2,007 QML files survive as readable source
  in `VECreator.dll`; the remainder are Qt bytecode units. Statements about those
  files rest on their paths, their imports and the C++ view-model tree, not on
  their contents.
- **The `Agent` (AI assistant) content surface.** It has 34 QML files and 108
  icons, which argues native, but no recovered source confirms whether the chat
  transcript itself is QML or an embedded view. It is listed as "likely native"
  above rather than certain.
- **Live panel data.** The `rp.db` normalised tables were empty on this install;
  everything reported about panels comes from the 361 cached HTTP responses
  across three of the four databases. A machine that had browsed more panels
  would show more. The panel-name list is therefore a floor, not a census.
- **Lynx bundle internals beyond strings.** The `template.js` bundles were read
  as byte streams and string-scanned. The `XMLH` (element tree) and `CSSI`
  (stylesheet) sections were identified by their section tags but not parsed, so
  the per-screen component composition of the Lynx cards is not described.
- **We did not run CapCut.** Everything above is static. No screen was observed
  rendering; every classification is inferred from files, symbols, strings and
  cached data.
