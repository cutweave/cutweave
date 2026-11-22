# Cutweave — CapCut Feature Parity Matrix

> Status: baseline 2026-11-22. Inputs: [research/01-capcut-features.md](research/01-capcut-features.md), [00-product-brief.md](00-product-brief.md), [01-architecture.md](01-architecture.md), [04-roadmap.md](04-roadmap.md), [05-work-breakdown.md](05-work-breakdown.md).
> Rows tagged **[U]** were "(unverified)" in the CapCut research. The former **(gap)** tags are resolved: each gap now has an epic home or is explicitly post-1.0 (roadmap "After 1.0"); see "Gaps found" (§13).

**Legend.**
- *Approach*: **Core** = engine command on the command bus (so UI, MCP, CLI and plugins all get it); **L0-L4** = plugin layer (arch §15: L0 data packs, L1 WGSL GPU effects, L1.5 compositions, L2 WASM, L3 UI panels, L4 native out-of-process); **AI** = gateway capability (arch §13) plus cloud provider and local fallback; **Comp** = built as a composition (arch §12).
- *Milestone*: M1 Skeleton, M2 Dogfood, M3 Public Alpha, M4 Creative parity, M5 AI Studio, M6 Beta, 1.0 (P7), or post-1.0. The milestone is where the feature is expected to be complete.
- *Priority*: **Must-1.0** (1.0 is not credible without it), **Should-1.0** (planned for 1.0, can slip to a point release), **Later** (post-1.0 or deferred), **Won't** (proprietary-service-bound; an open equivalent is named where one exists).
- Rows marked `(dup)` repeat a feature already listed in another category and are excluded from counts.

## 0. Summary

- **Total CapCut features tracked:** 131 (duplicates excluded; 43 tagged [U]).
- **By priority:** Must-1.0 71 / Should-1.0 35 / Later 17 / Won't 8.
- **By phase (primary epic, excluding Won't):** P1 12, P2 34, P3 15, P4 22, P5 29, P6 8 (3 rows have no epic).
- **By milestone:** M1 7, M2 37, M3 14, M4 18, M5 29, M6 9, post-1.0 17.
- **Coverage by 1.0:** 106 of 131 features = **81%** (Must 71 + Should 35 shipping by 1.0); 86% of the 123 non-Won't features. All Must-1.0 items are by definition covered. (Recomputed from the tables; the earlier "112 / 85%" figure did not match the row counts.)
- Gaps (§13, 25 tracked): 16 RESOLVED into existing epics, 8 Post-1.0 (roadmap "After 1.0"), 1 open (G25, effect-count parity). No tracked feature still depends on an unplanned epic.


---
## 1. Project and media management

| CapCut feature | Cutweave approach | Epic | Milestone | Priority | Notes |
|---|---|---|---|---|---|
| Media import | Core `media.import`; FFmpeg probe/index/decode | 1D | M1 | Must-1.0 | Drag-drop. Phone/QR transfer: Later (needs mobile companion) |
| Media library/bin | Core media index (SQLite), thumbnails; bin UI | 1D, 1M, 2F | M2 | Must-1.0 | Bin search, tags, ratings and metadata filters: RESOLVED → 2F (G12) |
| Stock video/photo library | L2 importer plugins: Pexels, Pixabay, Openverse (BYO key) | 5D, 5F (StockSearch) | M5 | Should-1.0 | Better than CapCut: no account, license shown per asset. G1 RESOLVED → 5D/5F `StockSearch` capability + stock adapters, results in the AI Studio/bin |
| Proxy mode | Core proxy jobs, auto policy, relink at export | 2F | M2 | Must-1.0 | Same render graph for preview and export |
| Preview resolution / HW accel toggles [8] | Core preview quality ladder; HW decode (a) path | 1H, 2H | M1 | Must-1.0 | Settings UI in 2H |
| Render cache [U] | Core frame cache + background cache of effect stacks | 2J | M2 | Should-1.0 | Frame cache in P1; background effect-stack render cache: RESOLVED → 2J (G11) |
| Project settings [U] | Core sequence settings (res/fps/ratio), cache dirs | 1A, 2B | M2 | Must-1.0 | |
| Drafts / autosave [U] | Core journal, atomic save, backups, crash recovery | 1C | M1 | Must-1.0 | Better than CapCut: command-journal recovery, open JSON format |
| Cloud drafts/sync | Local-first project folder; works with any folder sync (Syncthing/Dropbox). Optional L2 sync plugin later | — | post-1.0 | Later | Open equivalent, no account. Post-1.0 (roadmap 'After 1.0'; G14) |
| Brand kit / asset organisation [U] | Typed brand-kit data (fonts, colors, logo, caption style) + L0 pack; applied by copilot/skill `brand-kit-apply` | 6I, 6D | M6 | Should-1.0 | Memory in 6I; brand kit as L0 pack + inspector: RESOLVED → 6D (G13) |
| Team spaces | Won't (needs a hosted server). Open equivalent: shared folder / git-friendly project; real-time collab post-1.0 (Loro) | — | post-1.0 | Won't | Non-goal D1; revisit with CRDT collab |

## 2. Timeline editing

| CapCut feature | Cutweave approach | Epic | Milestone | Priority | Notes |
|---|---|---|---|---|---|
| Multi-track timeline | Core model: unlimited video/audio/overlay tracks | 1A, 2A | M2 | Must-1.0 | |
| Magnetic main track | Core ripple-by-default track flag | 2A | M2 | Must-1.0 | Toggle like Track Magnet |
| Auto snapping | Core snap-point index (playhead, edges, markers, beats) | 2A | M2 | Must-1.0 | |
| Split / trim / delete | Core `timeline.*` commands; JKL, frame step | 1B, 2A | M1 | Must-1.0 | Better: per-actor undo, agent-drivable |
| Ripple delete / trim [U] | Core ripple, roll, slip, slide | 2A | M2 | Must-1.0 | More than CapCut |
| Compound clips [U] | Core nested sequence as a clip | 2B | M2 | Should-1.0 | Sequence-as-clip: RESOLVED → 2B (G6) |
| Markers [U] | Core markers, in/out; chapters export | 2A | M2 | Must-1.0 | Marker-set export in 2G |
| Link/unlink audio [U] | Core linked clips | 2A | M2 | Must-1.0 | |
| Multi-cam | Deferred per roadmap; lite version = audio cross-correlation sync + angle switch command | 2A | post-1.0 | Later | Pro-locked in CapCut. YouTube podcast users will ask. Post-1.0 (roadmap 'After 1.0'; G3) |
| Speed 0.1x-100x | Core constant speed, pitch preservation | 2D | M2 | Must-1.0 | |
| Speed curves | Core retime curves + L0 presets (montage, hero, bullet, flash) | 4H | M4 | Should-1.0 | |
| Optical flow slow-mo | Core frame blending (basic); AI `Interpolate` for quality | 4H, 5G | M5 | Should-1.0 | Local RIFE via 5I where cheap |
| Reverse [U] | Core reverse | 2D | M2 | Must-1.0 | |
| Freeze frame [U] | Core freeze | 2D | M2 | Must-1.0 | |
| Replace clip [U] | Core `clip.replace_media` keeping timing/effects | 2A | M2 | Should-1.0 | RESOLVED → 2A (G7) |
| Shortcut remap [8] | Core command registry; shortcut editor; CapCut preset | 2H | M2 | Must-1.0 | Better: every plugin command bindable |
| Track lock/mute/hide [U] | Core track flags; solo | 2A | M2 | Must-1.0 | |

## 3. Canvas and transform

| CapCut feature | Cutweave approach | Epic | Milestone | Priority | Notes |
|---|---|---|---|---|---|
| Aspect ratios [U] | Core sequence presets 16:9, 9:16, 1:1, 4:5, 21:9; custom | 2B | M2 | Must-1.0 | |
| Background fill [U] | Core: color, blur, image, gradient | 2B | M2 | Must-1.0 | Gradient not listed in P2 scope; trivial add |
| Crop/rotate/flip/mirror [U] | Core clip transform props | 2C | M2 | Must-1.0 | |
| Position/scale/rotate/opacity [8] | Core transform + on-canvas handles | 2C, 2H | M2 | Must-1.0 | |
| Keyframes | Core keyframes on all animatable props, easing, lanes | 2C | M2 | Must-1.0 | |
| Keyframe graph editor | Timeline/UI curve editor (bezier) | 2C, 2H | M2 | Should-1.0 | Curve (graph) editor: RESOLVED → 2C (G9) |
| Masks | L1 mask generator; shape, feather, invert, keyframable | 4G | M4 | Must-1.0 | AI click-to-track masks in 5G |
| Blend modes | L1 blend modes (basic set in core) | 2C, 4A | M2 | Must-1.0 | More via L1 plugins |
| Chroma key | L1 keyer with spill suppression; luma key | 4G | M4 | Must-1.0 | |
| Picture-in-picture | Core overlay tracks + transform | 2A, 2C | M2 | Must-1.0 | |
| Stabilization | Core analysis job (vidstab) + warp | 4I | M4 | Should-1.0 | |
| Motion/camera tracking | Core point/planar-lite tracker driving keyframes; AI `SegmentTrack` | 4I | M4 | Should-1.0 | Text/sticker follow |
| Auto reframe | AI/Core: detection + smoothing to keyframed crop; local ONNX fallback | 5G | M5 | Must-1.0 | Core to short-form; manual reframe M2 |
| Auto cutout / remove BG | AI `Matting` (cloud) + local ONNX matting (5I) | 5G, 5I | M5 | Must-1.0 | Better: works offline. Avoid RMBG-2.0 license trap |

## 4. Color

| CapCut feature | Cutweave approach | Epic | Milestone | Priority | Notes |
|---|---|---|---|---|---|
| Basic adjust | Core GPU color ops (exposure, contrast, temp, tint, sat, vibrance...) | 4F | M4 | Must-1.0 | Linear-light float pipeline |
| HSL | Core HSL per hue | 4F | M4 | Must-1.0 | |
| Curves | Core RGB + per-channel curves | 4F | M4 | Must-1.0 | |
| Color wheels | Core lift/gamma/gain (basic) | 4F | M4 | Should-1.0 | Not Pro-locked, unlike CapCut |
| LUTs [U] | L0 LUT packs, `.cube` import | 4D, 4F | M4 | Must-1.0 | |
| Filters | L0 packs (>= 40 filters/LUTs) | 4D | M4 | Must-1.0 | |
| Auto adjust / AI color | Core histogram heuristics + `color.auto` command; LLM/vision via skill | 4F | M4 | Should-1.0 | `color.auto`: RESOLVED → 4F (G8) |
| AI color match [U] | Core color-transfer command; agent skill `color-and-look-matching` | 4F, 6G | M6 | Should-1.0 | Skill in 6G; engine color-transfer primitive: RESOLVED → 4F (G8) |
| Scopes [U] | Core GPU waveform/vectorscope/histogram | 4F | M4 | Should-1.0 | Free, unlike CapCut Pro |
| HDR [U] | Tone-map HDR sources to SDR in 1.0; HDR output v2 | 1D, 2G | post-1.0 | Later | Arch: HDR output is v2. Ingest tone-map is Must |

## 5. Effects and transitions

| CapCut feature | Cutweave approach | Epic | Milestone | Priority | Notes |
|---|---|---|---|---|---|
| Video effects | L1 WGSL effects; >= 50 first-party | 4A, 4C | M4 | Must-1.0 | CapCut has hundreds; plugin ecosystem closes the count gap |
| Body effects [U] | L1 effect + AI `Matting` person mask input | 4C, 5G | M5 | Later | Composition of effects + matting; not a P4 deliverable |
| Face effects/AR [U] | Needs face-landmark model (local ONNX) feeding L1 effects | 5I | post-1.0 | Later | No face-landmark capability in scope. Post-1.0 (roadmap 'After 1.0'; G2) |
| Transitions | L1 transitions; >= 40 first-party incl. light leaks | 4A, 4B | M4 | Must-1.0 | |
| Animations in/out/combo [U] | L0/L1.5 preset keyframe animations | 4E | M4 | Must-1.0 | |
| Adjustment layers [U] | Core adjustment-layer item type | 4G | M4 | Must-1.0 | |
| Remove flicker | L1 temporal deflicker effect (multi-frame input) | 4C | post-1.0 | Later | Post-1.0 (roadmap 'After 1.0'; G5) |

## 6. Text and captions

| CapCut feature | Cutweave approach | Epic | Milestone | Priority | Notes |
|---|---|---|---|---|---|
| Text styles/fonts [U] | Core text engine (HarfBuzz/MSDF); bundled OFL fonts + system | 3A | M3 | Must-1.0 | Moat. Font licensing: OFL only bundled |
| Text templates/presets | L0 text preset packs | 3A, 3C | M3 | Must-1.0 | Moat |
| Text animations [U] | L1.5 compositions; per-glyph/word/line animators | 3B, 3C | M3 | Must-1.0 | Better: code-defined, extensible |
| Curved text [U] | Core text-on-path layout | 3A | M3 | Should-1.0 | Text-on-path: RESOLVED → 3A (G4) |
| Auto captions | AI `Stt`: cloud (ElevenLabs/Deepgram) + local whisper.cpp; transcript-first | 3D, 3E | M3 | Must-1.0 | Better: free, offline-capable, no account |
| Caption styles + keyword highlight | L0 styles + L1.5 comps; >= 25 styles, active-word, karaoke, pop, emoji | 3E | M3 | Must-1.0 | Moat |
| Speaker-ID captions | `Stt` diarization labels; per-speaker style | 3D, 3E | M3 | Should-1.0 | Labels in transcript model; per-speaker styling to confirm in 3E |
| Bilingual captions [U] | Translate capability, bilingual layout | 3E, 5G | M5 | Should-1.0 | Layout in 3E; MT via `Translate`/LLM |
| Caption translation / video translator | AI `Translate` (LLM or DeepL-class) + `Dub` | 5G | M5 | Should-1.0 | Skill `dubbing-and-translation` |
| Lyrics recognition [U] | `Stt` with alignment mode, LRC output | 3D | M3 | Later | RESOLVED → 3D (G15; low priority, may slip) |
| Subtitle import/export [U] | SRT/VTT/ASS (core); LRC via L2 importer | 3E, 6E | M3 | Must-1.0 | LRC small add |
| Text-to-speech | AI `Tts`: cloud (ElevenLabs etc.); local Piper/Kokoro fallback | 5G, 5I | M5 | Must-1.0 | Local TTS fallback (Kokoro): RESOLVED → 5I (G10) |
| Voice cloning | AI `VoiceClone` (cloud, consent gate). XTTS/F5 excluded by license | 5G | M5 | Later | Consent + provenance required |

## 7. Stickers, overlays, templates

| CapCut feature | Cutweave approach | Epic | Milestone | Priority | Notes |
|---|---|---|---|---|---|
| Stickers | L0 sticker packs (static/animated), emoji; GIPHY-alternative via L2 importer | 3G | M3 | Must-1.0 | Animated formats (Lottie/APNG/WebP/GIF): RESOLVED → 3G (G16) |
| AI stickers | AI `TextToImage` + `Matting`, saved as sticker asset | 5F | M5 | Later | Composition of existing capabilities |
| Overlays [U] | L0 overlay packs (blend-mode assets), light leaks, frames | 3G, 4B | M3 | Must-1.0 | |
| Template marketplace | Template format + git-based plugin index; >= 30 first-party templates | 6D, 6C | M6 | Must-1.0 | Open equivalent: community index, no hosting fee, no rev-share |
| Personalised templates | Smart templates: slot auto-fill from media bin; copilot | 6D, 6I | M6 | Should-1.0 | |
| Skills Marketplace (Oct 2026) | Plugin index + Agent Kit skills/prompt packs | 6C, 6G | M6 | Should-1.0 | Open and git-based; vendor-neutral |
| Design Studio 2.0 | Won't: separate infinite-canvas product. Compositions cover title/graphic design inside a video | 3B | post-1.0 | Won't | Out of scope; not a video editor feature |

## 8. Audio

| CapCut feature | Cutweave approach | Epic | Milestone | Priority | Notes |
|---|---|---|---|---|---|
| Music library | L0 music packs (CC0/FMA), AI `Music` | 3G, 5F | M3 | Must-1.0 | License shown per track |
| SFX library | L0 SFX packs; AI `Sfx` | 3G, 5F | M3 | Must-1.0 | |
| Voice effects/changer [U] | AI `VoiceConvert`; local pitch/formant DSP effects | 5G, 2E | M5 | Should-1.0 | Offline DSP voice FX: RESOLVED → 2E (G17) |
| Noise reduction / audio enhancer | AI `AudioEnhance` cloud + local RNNoise/DeepFilterNet | 5G, 5I | M5 | Must-1.0 | Better: offline |
| Voice enhance [U] | AI `AudioEnhance` | 5G | M5 | Should-1.0 | |
| Vocal isolation | AI `StemSplit` cloud; local Demucs optional | 5G, 5I | M5 | Should-1.0 | Local stem split not in 5I list (minor) |
| Beat detection / sync [U] | Local ONNX/DSP beat detect to markers; snap | 5G, 5I | M5 | Should-1.0 | Beat snapping in 2A needs it earlier |
| Auto ducking [U] | Core VAD + gain envelope command | 2E | M2 | Must-1.0 | Auto-ducking (`audio.duck`): RESOLVED → 2E (G18) |
| Fades, volume keyframes [U] | Core gain curves, fades, pan, buses | 2E | M2 | Must-1.0 | |
| Extract audio [U] | Core `media.extract_audio` | 2E | M2 | Must-1.0 | |
| Recording/voiceover [U] | Core audio capture to `generated/` | 2E | M2 | Must-1.0 | Voiceover in M2. Screen/webcam recording: Post-1.0 (roadmap 'After 1.0'; G19) |
| Loudness normalization | Core EBU R128 presets | 2E | M2 | Must-1.0 | Also for export targets |
| AI music generation | AI `Music` via BYO providers | 5F | M5 | Should-1.0 | |
| Copyright check | Won't: needs a proprietary fingerprint DB. Open equivalent: per-asset license metadata + provenance sidecar | 3G, 5A | M5 | Won't | Per-asset `license` field in packs: RESOLVED → 3G (G20) |

## 9. AI features

| CapCut feature | Cutweave approach | Epic | Milestone | Priority | Notes |
|---|---|---|---|---|---|
| (dup) Auto captions | see §6 | 3E | M3 | Must-1.0 | |
| (dup) Background removal / auto cutout | see §3 | 5G | M5 | Must-1.0 | |
| (dup) Auto reframe | see §3 | 5G | M5 | Must-1.0 | |
| Enhance quality / upscale | AI `Upscale` cloud; local Real-ESRGAN via ONNX | 5G, 5I | M5 | Should-1.0 | |
| Frame interpolation | AI `Interpolate`; local RIFE optional | 5G | M5 | Should-1.0 | |
| Retouch / beauty | Face-landmark model + L1 retouch effect | 5I | post-1.0 | Later | Post-1.0 (roadmap 'After 1.0'; G2) |
| AI avatars/characters | `Tts` + `LipSync` / image-to-video; compose in AI Studio | 5F, 5G | M5 | Later | Provider-driven; consent/provenance gates |
| AI writer / script to video | LLM + `TextToVideo`/stock retrieval; skills `script-to-storyboard`, `generate-shots` | 6G, 5J | M6 | Should-1.0 | Better: agent-driven, any LLM |
| Long video to shorts | `Stt` + `VideoUnderstand` + reframe, ranked candidates as sequences | 5H | M5 | Must-1.0 | Headline short-form feature |
| Filler word / silence removal | Transcript-based `ripple_delete_ranges` with review UI | 3F | M3 | Must-1.0 | Free, offline with whisper.cpp |
| Video translator / dubbing / lip-sync | AI `Translate` + `Tts` + `Dub` + `LipSync` (optional) | 5G | M5 | Should-1.0 | |
| AI image generator | AI `TextToImage`/`ImageEdit` via aggregators and direct | 5F | M5 | Must-1.0 | Any model, BYO keys |
| AI video generation | AI `TextToVideo`/`ImageToVideo` (fal, Replicate, Runway, Higgsfield...) | 5C, 5D, 5F | M5 | Must-1.0 | Cost shown up front; no credits tier |
| AI Edit / AI Extend | AI `VideoToVideo`; "extend clip" and "fill gap" | 5F | M5 | Should-1.0 | |
| EditPilot (natural-language edits) | Copilot over same tool catalog; also any external agent via MCP | 6I, 1J, 6J | M6 | Must-1.0 | Better: bring your own agent, per-actor undo |
| Generative/Interactive Edit | AI `VideoToVideo` with region mask + prompt | 5G | post-1.0 | Later | Region-prompt edit: Post-1.0 (roadmap 'After 1.0'; G21) |
| Agentic creation (pilot) | Agent Kit skills + MCP prompts + copilot | 6G, 6I | M6 | Must-1.0 | Core differentiator |
| AI Lab (mobile) | Won't (mobile app) | — | post-1.0 | Won't | Desktop-only v1 (D1) |
| Relight [U] | AI `VideoToVideo` / depth-normal relight via provider | 5G | post-1.0 | Later | Opportunistic via providers |
| Eye contact [U] | Face model (local/cloud) | 5I | post-1.0 | Later | Post-1.0 (roadmap 'After 1.0'; G2) |
| AI movement / stabilize / flicker | Tracking + stabilization + deflicker | 4I, 4C | M4 | Should-1.0 | Flicker: Post-1.0 (roadmap 'After 1.0'; G5) |
| Image: text/people remover, inpaint | AI `ImageEdit` (cloud); local LaMa optional | 5F | M5 | Later | Provider-supplied |
| AI transparency checker | Core C2PA read/write + provenance sidecars | 5A | M5 | Should-1.0 | Better: provenance for every AI asset |
| ChatGPT plugin | Native MCP server (any MCP client: ChatGPT, Claude, Codex, Gemini CLI) | 1J, 6J | M1 | Must-1.0 | Better: full control, not a plugin |
| AI model/product features | Won't as a bundle (sibling apps Pippit/Hypic); provider capabilities cover generation | 5F | post-1.0 | Won't | Proprietary product suite |

## 10. Export and publishing

| CapCut feature | Cutweave approach | Epic | Milestone | Priority | Notes |
|---|---|---|---|---|---|
| Resolution | Core export up to 4K+ (no paywall); GPU scaler | 1E, 2G | M2 | Must-1.0 | Better: no tier limits |
| Frame rate | Core fps conform | 2G | M2 | Must-1.0 | |
| Codecs/containers | H.264/AAC (M1); HEVC, AV1, ProRes, MOV/MKV/WebM | 1E, 2G | M2 | Must-1.0 | Patent posture review in 7F |
| HDR export [U] | HDR output is v2 | — | post-1.0 | Later | Arch §22 |
| Bitrate control [U] | Core rate-control settings in export dialog | 2G | M2 | Should-1.0 | Advanced export (bitrate, 2-pass, color tags): RESOLVED → 2G (G22) |
| GIF export [U] | Core palettegen export | 2G | M2 | Must-1.0 | |
| Audio-only export [U] | Core WAV/MP3/AAC/FLAC/Opus | 2G | M2 | Must-1.0 | |
| Direct upload | L2 publisher plugins: YouTube, TikTok (OAuth in engine) | 6F | M6 | Should-1.0 | Keys/tokens stay local; LLM-written metadata |
| Watermark | None, ever | — | M1 | Must-1.0 | Better than CapCut (positioning point) |
| Multi-version export | Export queue, marker sets, per-platform presets; `export-for` prompt | 2G | M2 | Should-1.0 | Brand variants via templates |

## 11. Collaboration, cloud, accounts

| CapCut feature | Cutweave approach | Epic | Milestone | Priority | Notes |
|---|---|---|---|---|---|
| Account required | No account; no telemetry by default; keys in OS keychain | 1L, 5A | M1 | Must-1.0 | Better than CapCut |
| Cloud storage | Won't (hosted service). Open equivalent: user's own storage, Collect & Archive | 2F | post-1.0 | Won't | Non-goal: no hosted inference/storage |
| Team workspaces | Won't (hosted). Open equivalent: shared folder/git + later CRDT collab | — | post-1.0 | Won't | See §1 |
| Cross-device continue | Portable project folder; mobile companion post-1.0 | 2F | post-1.0 | Later | |
| Credits | Replaced by BYO keys and up-front cost estimates/budgets | 5A | M5 | Won't | No Cutweave credits system |
| Offline | Local-first; local provider plugin (whisper.cpp, ONNX) | 5I | M5 | Must-1.0 | Better than CapCut |

---
## 12. Cutweave-only differentiators

| Differentiator | CapCut | Cutweave approach | Epics |
|---|---|---|---|
| Full MCP agent control, no screenshots | None (one ChatGPT plugin) | 100% of commands as MCP tools; `render_frame`, contact sheets, checkpoints | 0H, 1J, 2I, 6J |
| Agent Kit (skills, prompts, evals) | None | ~14 skills, MCP prompts, nightly eval harness, install guides | 3H, 5J, 6G, 6H |
| Plugins at every layer | None on desktop | L0 data, L1 WGSL, L1.5 compositions, L2 WASM, L3 UI, L4 later | 4A, 6A, 6B, 6C |
| Code-defined compositions | None | TSX to QuickJS scenes, `@cutweave/compose`, inspector from props schema | 0G, 3B |
| Any-model AI via BYO keys/aggregators | Closed models, credits | Capability gateway, signed catalog, fal/Replicate/OpenRouter/Higgsfield, user-added slugs | 5A-5E |
| Adapter SDK for new providers | None | Declarative HTTP manifest + WASM adapters | 5E |
| Offline / local-first AI | AI needs internet and account | Local provider plugin with model manager and license display | 5I |
| Open project format + OTIO | Proprietary draft | Documented JSON project, OTIO import/export, FCPXML/EDL | 1C, 6E |
| No account, no telemetry, no watermark, no tiers | Account, Pro paywall, watermark | Keychain keys, opt-in crash reports, no feature gating | 1L, 3I, 7F |
| Per-actor undo and replayable command logs | None | Actor-tagged history, command journal, reproducible agent sessions | 1B, 6I |
| Copilot with plan, preview, apply | EditPilot (credits) | Provider-agnostic loop, cost and destructive-action gates | 6I |
| Provenance and cost transparency | Credits opaque | Provenance sidecars, C2PA, estimates, budgets | 5A |
| Headless CLI and CI rendering | None | `cutweave exec/query/render`, deterministic render for batch | 1K, 2G |
| Open caption/template/effect ecosystem | Closed marketplace, revenue-gated | Git-based plugin index, signed, in-app install | 6C, 6D |
| Crash-safe journal recovery | Local drafts | Journal replay, rotating backups | 1C, 7B |

---
## 13. Gaps found

Each row was a CapCut feature (or supporting need) whose natural home was not in the roadmap scope bullets or epic names. Status after integration: **RESOLVED → epic** (16 gaps: G1, G4, G6–G13, G15–G18, G20, G22), **Post-1.0** per the roadmap's "After 1.0" (8 gaps: G2, G3, G5, G14, G19, G21, G23, G24), **open** (1 gap: G25).

| ID | CapCut feature / need | Why it is a gap | Suggested epic / phase | Pri |
|---|---|---|---|---|
| G1 | Stock video/photo library | Only CC0 sticker/music packs in 3G; no stock provider kind | RESOLVED → 5D/5F (`StockSearch` capability + stock adapters; no 6C importer kind needed) | Should |
| G2 | Face/body effects, AR, beauty retouch, eye contact | No face-landmark/segmentation capability; closed enum lacks `FaceLandmarks`/`Retouch` | Post-1.0 (roadmap 'After 1.0'); was: Add capability + local ONNX model to 5I (post-1.0; license-check models, avoid insightface) | Later |
| G3 | Multi-cam | Roadmap defers to after 1.0; M2 exit says "multi-cam-free" | Post-1.0 (roadmap 'After 1.0'); was: Lite in P6: sync-by-audio (2F) + angle-switch command (2A) | Later |
| G4 | Curved text / text on path | Not in 3A text engine bullets | RESOLVED → 3A | Should |
| G5 | Remove flicker / temporal deflicker | Not in P4 effects list; needs multi-frame input in 4A | Post-1.0 (roadmap 'After 1.0'); was: 4C after 4A multi-frame inputs; post-1.0 | Later |
| G6 | Compound clips | P2 has sequences and nested selection, not sequence-as-clip | RESOLVED → 2B | Should |
| G7 | Replace clip (keep timing/effects) | Not named in P2 | RESOLVED → 2A | Should |
| G8 | Auto color adjust, AI color match | Skill exists in 6G but no engine primitive in 4F | RESOLVED → 4F | Should |
| G9 | Keyframe graph (bezier) editor | P2 lists easing curves and lanes, not a curve editor UI | RESOLVED → 2C | Should |
| G10 | Local TTS fallback (Piper/Kokoro) | 5I lists whisper, matting, denoise, scene/beat only | RESOLVED → 5I | Should |
| G11 | Background render cache of effect stacks | Only a frame cache in P1; not in P2 perf scope | RESOLVED → 2J | Should |
| G12 | Media bin search, tags, ratings, metadata filters | P1 bin and 2F cover import/proxy/relink only | RESOLVED → 2F | Should |
| G13 | Brand kit UI/data pack in the editor | Only copilot memory (6I) and skill; no UI | RESOLVED → 6D | Should |
| G14 | Cloud/cross-device sync | Local-first only; collab post-1.0 | Post-1.0 (roadmap 'After 1.0'); was: Docs recipe for folder sync in 7E; optional L2 sync plugin post-1.0 | Later |
| G15 | Lyrics recognition and LRC | Stt lacks lyric-alignment mode; LRC format unlisted | RESOLVED → 3D | Later |
| G16 | Animated sticker formats (Lottie, APNG, animated WebP, GIF) | 3G says packs; decoder support unscoped | RESOLVED → 3G | Should |
| G17 | Offline voice effects (pitch/formant/robot) | Only `VoiceConvert` (cloud) | RESOLVED → 2E | Later |
| G18 | Auto ducking | Eval suite assumes it; no epic scope in P2/P3 | RESOLVED → 2E | Must |
| G19 | Screen/webcam recording | CapCut desktop has it [U, not in research]; roadmap only has voiceover | Post-1.0 (roadmap 'After 1.0'); was: 2E extension or post-1.0 | Later |
| G20 | Music copyright safety (license metadata) | CapCut "copyright check" is Won't; no license field in pack spec | RESOLVED → 3G | Should |
| G21 | Region-select generative edit / inpaint | `VideoToVideo` exists, no UI/flow in 5F | Post-1.0 (roadmap 'After 1.0'); was: 5F/5G post-1.0 | Later |
| G22 | Advanced export controls (bitrate, 2-pass, color tags) | Presets only | RESOLVED → 2G | Should |
| G23 | Phone-to-desktop QR import | Mobile out of scope | Post-1.0 (roadmap 'After 1.0'); was: post-1.0 with mobile companion | Later |
| G24 | HDR export | Roadmap defers HDR output (arch §22) | Post-1.0 (roadmap 'After 1.0'); was: post-1.0; ingest tone-map stays in 1D | Later |
| G25 | Effect count parity (CapCut hundreds vs 50 effects/40 transitions) | Numbers in P4 | Closed by community plugins; track effect count after 1.0 | Later |

---
## 14. Coming from CapCut: terminology

| CapCut term | Cutweave term |
|---|---|
| Draft / Project | Project (`.cutweave` folder); autosave via journal |
| Media panel / Library | Media bin |
| Main track / Track Magnet | Main track with ripple-by-default (magnetic) |
| Overlay / PiP track | Overlay track |
| Compound clip | Nested sequence (compound clip) |
| Canvas / ratio | Sequence settings (aspect preset) |
| Keyframe | Keyframe (lane in timeline, curve editor) |
| Effects / Video effects | Effects (L1 WGSL plugins) |
| Filters | Filters (LUT packs) |
| Transitions | Transitions (L1 plugins) |
| Animations (in/out/combo) | Clip animations |
| Text template / Text preset | Text preset / caption style (L0 + compositions) |
| Auto captions | Captions (transcript-first, `Stt` capability) |
| Smart cut / Remove silence | Silence and filler removal (text-based editing) |
| Stickers / Elements | Stickers and overlays (asset packs) |
| Templates / Marketplace | Templates / plugin index |
| Auto reframe | Auto reframe (AI feature) |
| Remove background / Auto cutout | Matting (smart cutout) |
| AI credits | Provider cost estimates and budgets (BYO keys) |
| EditPilot / AI agent | Copilot, or any MCP agent |
| Pro / watermark | None: all features free, no watermark |
| Cloud space | Your own folder / storage |
| Export | Export (presets, queue) |
| Plugins (none) | Plugins (L0-L4) |
