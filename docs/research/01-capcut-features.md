# CapCut Feature Inventory (for Cutweave parity planning)

Date: 2026-11-22. Method: web search + fetch of CapCut official pages, newsroom and third-party reviews.

**Legend.** Platforms: D=Desktop (Win/macOS), M=Mobile (iOS/Android), W=Web. Tier: F=Free, P=Pro (or credit-gated), F/P=basic free, premium assets/options gated. Complexity to rebuild: S/M/L/XL.
**Verification.** Rows with a source tag [n] (see Sources) were confirmed by a fetched page. Rows without a tag come from prior product knowledge and were NOT re-verified this session: treat as "(unverified)". Gating changes often; CapCut says new Pro features roll out via updates, and mobile usually gets AI features before desktop [9].

---
## 0. Plans and gating summary
- Free: full multi-track timeline, keyframes, chroma key, basic AI voiceover, free music/SFX; Pro-marked templates/effects/fonts export with watermark; export 1080p [4][9]. Conflict: one CapCut page says the free version exports 1080p and 4K without watermark, others say 1080p cap + watermark (which holds today: unverified) [8][9].
- 2025 restructure reportedly split paid into Standard (~$9.99/mo, watermark removed, no 4K) and Pro (~$19.99/mo, $179.99/yr, 4K, full AI toolkit, 100GB+ cloud) [4]; CapCut's own comparison page says Pro up to 8K, ~1024GB, team from ~$24.99/mo [3]. Older Pro price was $9.99/mo ($77/yr) [9].
- Ultra tier + credits: AI Video/Edit/Extend (Seedance 2.5, Aug 2026) is for Pro and Ultra members and consumes credits billed per second [7].
- 1 GB free cloud storage discontinued Aug 2024 [3]. Desktop Pro-locked examples: multi-cam, advanced color grading [11].
- AI tools require internet and an account; no third-party plugin support on desktop [9].

---
## 1. Project and media management
| Feature | Description | Plat | Tier | Cx | Implementation hint |
|---|---|---|---|---|---|
| Media import | Video/audio/image from disk, drag-drop, phone/QR | D/M/W | F | S | FFmpeg/libav demux, thumbnail cache |
| Media library/bin | Local/Cloud/Library tabs, thumbnails, search | D/M/W | F | M | SQLite index, thumbnailer |
| Stock video/photo library | Royalty-free footage, GIPHY integration [8] | D/M/W | F/P | M | Needs asset backend; Cutweave: Pexels/Pixabay/Openverse plugins |
| Proxy mode | Edit low-res copies, originals at export [8] | D | F | M | FFmpeg proxy transcode, relink on export |
| Preview resolution / HW accel toggles [8] | Lower preview res, GPU toggle | D | F | M | NVDEC/VideoToolbox/VA-API decode |
| Render cache | Background cache of effect stacks (unverified) | D | F | L | Frame cache, GPU compositor |
| Project settings | Resolution, fps, ratio, cache dir (unverified) | D/M | F | S | Project schema |
| Drafts / autosave | Local drafts, recovery (unverified) | D/M | F | S | JSON project + autosave |
| Cloud drafts/sync | Projects sync desktop<->mobile [9] | D/M/W | account; storage P [3] | L | Local-first: optional WebDAV/P2P sync |
| Brand kit / asset organisation | Brand kits, export versions [7] | D | P (unverified) | M | Asset tagging |
| Team spaces | Shared scripts, refs, footage, feedback [7] | D/W | P | XL | Needs server; out of local-first scope |

## 2. Timeline editing
| Feature | Description | Plat | Tier | Cx | Hint |
|---|---|---|---|---|---|
| Multi-track timeline | Stacked video/audio/text/sticker layers [8] | D/M/W | F | L | Track/clip model, compositor |
| Magnetic main track | Auto-close gaps; Track Magnet toggle [8] | D/M/W | F | M | Ripple-by-default logic |
| Auto snapping | Snap to edges, playhead, markers [8] | D/M/W | F | S | Snap-point index |
| Split / trim / delete | Ctrl/Cmd+B split, frame stepping, shortcuts [8] | D/M/W | F | S | Edit ops + undo stack |
| Ripple delete / trim | Close gaps (unverified) | D/M | F | M | Timeline ops |
| Compound clips | Nest clips (unverified) | D/M | F | L | Nested sequences |
| Markers | Chapter/beat markers (unverified) | D/M | F | S | Marker list |
| Link/unlink audio | Linked A/V (unverified) | D/M | F | S | Clip groups |
| Multi-cam | Sync angles, switch (was Pro-locked [11]) | D | P | L | Audio cross-correlation sync |
| Speed 0.1x-100x | Constant speed, pitch option [9] | D/M/W | F | M | Time-stretch (rubberband) |
| Speed curves | Bezier ramps, presets [9] | D/M | F/P | L | Variable retime + audio handling |
| Optical flow slow-mo | Smooth slow-mo [8] | D/M | F/P | XL | RIFE / optical flow |
| Reverse | Reverse clip (unverified) | D/M/W | F | S | Reverse decode w/ GOP cache |
| Freeze frame | Hold frame (unverified) | D/M/W | F | S | Frame extract |
| Replace clip | Swap media keep timing (unverified) | D/M | F | S | Reference swap |
| Shortcut remap [8] | Custom keybindings | D/W | F | S | Command registry |
| Track lock/mute/hide | Track controls (unverified) | D/M | F | S | Track flags |

## 3. Canvas and transform
| Feature | Description | Plat | Tier | Cx | Hint |
|---|---|---|---|---|---|
| Aspect ratios | 16:9, 9:16, 1:1, 4:3, etc. (unverified list) | D/M/W | F | S | Canvas size |
| Background fill | Color, image, gradient, blur (unverified) | D/M/W | F | M | GPU blur shader |
| Crop/rotate/flip/mirror | (unverified) | D/M/W | F | S | Transform matrix |
| Position/scale/rotate/opacity | Inspector transforms [8] | D/M/W | F | S | Compositor |
| Keyframes | Position, scale, opacity, rotation, effects [8][9] | D/M/W | F | L | Param animation system |
| Keyframe graph editor | Bezier curves; desktop only, web lacks it [9] | D | F | L | Curve editor UI |
| Masks | Shape masks, feather; keyframable [9] | D/M/W | F | M | Mask SDF shader |
| Blend modes | (listed [9]) | D/M | F | M | Blend shaders |
| Chroma key | Green screen, intensity, shadow [8] | D/M/W | F | M | HSV/YCbCr key + spill |
| Picture-in-picture | Overlay tracks (unverified) | D/M/W | F | M | Multi-layer compositing |
| Stabilization | Adjustable strength [9] | D/M | F/P | L | Feature tracking + warp |
| Motion/camera tracking | Text/sticker follows subject [4][8] | D/M | P | L | Point/planar tracker (OpenCV) |
| Auto reframe | Subject-following crop [8][9] | D/M/W | F/P | L | Detection + smoothing |
| Auto cutout / remove BG | One-click person removal [8][12] | D/M/W | F/P | XL | Video matting (RVM/SAM2) |

## 4. Color
| Feature | Description | Plat | Tier | Cx | Hint |
|---|---|---|---|---|---|
| Basic adjust | Brightness, contrast, saturation, temp, etc. [8] | D/M/W | F | M | GPU shader pipeline |
| HSL | Per-hue [8][9] | D/M | F | M | HSL shader |
| Curves | Curve editors [9] | D/M | F | M | LUT from curve |
| Color wheels | Lift/gamma/gain [9] | D | F/P (advanced grading Pro-locked [11]) | M | Shader |
| LUTs | .cube import (unverified) | D/M | F | M | 3D LUT sampling |
| Filters | Cinematic library [8] | D/M/W | F/P | M | LUT + asset library |
| Auto adjust / AI color | Balance light/color across clips [12] | D/M | F/P | M | Histogram heuristics |
| AI color match | (unverified) | D | P | L | Color transfer |
| Scopes | Waveform/vectorscope (unverified) | D | P | M | GPU histogram |
| HDR | (unverified) | D/M | P | XL | OCIO, PQ/HLG |

## 5. Effects and transitions
| Feature | Description | Plat | Tier | Cx | Hint |
|---|---|---|---|---|---|
| Video effects | Hundreds of glitch/blur/light effects (unverified count) | D/M/W | F/P | L | Shader plugin API (GLSL/WGSL) |
| Body effects | Person-aware (unverified) | D/M | F/P | L | Person segmentation |
| Face effects/AR | (unverified) | M/D | F/P | L | Face landmarks |
| Transitions | Larger desktop set incl. light leaks, 3D zoom [8] | D/M/W | F/P | M | Two-input shaders |
| Animations in/out/combo | Clip and text animations (unverified) | D/M/W | F/P | M | Preset keyframes |
| Adjustment layers | (unverified) | D/M | F | M | Layer model |
| Remove flicker | [12] | D/M | P | L | Temporal deflicker |

## 6. Text and captions
| Feature | Description | Plat | Tier | Cx | Hint |
|---|---|---|---|---|---|
| Text styles/fonts | Fonts, stroke, shadow, glow, bg (unverified) | D/M/W | F/P | M | HarfBuzz; font licensing |
| Text templates/presets | AI text presets [12] | D/M/W | F/P | M | Template format |
| Text animations | In/out/loop (unverified) | D/M/W | F/P | M | Per-glyph animation |
| Curved text | (unverified) | D/M | F | M | Path layout |
| Auto captions | 16+ languages [8] | D/M/W | P (presets mostly Pro [9]) | L | Whisper local ASR |
| Caption styles + keyword highlight | [8] | D/M/W | P | M | Word timestamps + style engine |
| Speaker-ID captions | [4] | D | P | L | Diarization |
| Bilingual captions | (unverified) | D/M | P | M | MT |
| Caption translation / video translator | [12] | D/M/W | P | L | NLLB/Marian MT |
| Lyrics recognition | (unverified) | M/D | F/P | L | ASR + alignment |
| Subtitle import/export | SRT/LRC (unverified) | D/M | F | S | Parsers |
| Text-to-speech | 1000+ voices, tones [8][12]; basic free [4] | D/M/W | F/P | L | Local TTS (Piper/Kokoro) |
| Voice cloning | [8] | D | P | XL | XTTS/F5; consent issues |

## 7. Stickers, overlays, templates
| Feature | Description | Plat | Tier | Cx | Hint |
|---|---|---|---|---|---|
| Stickers | Static/animated, GIPHY [8] | D/M/W | F/P | M | Asset licensing; Lottie/WebP |
| AI stickers | [12] | M/D | P (unverified) | L | Diffusion model |
| Overlays | Light leaks, frames (unverified) | D/M/W | F/P | S | Blend-mode assets |
| Template marketplace | Creator templates (unverified detail) | M/D/W | F/P | XL | Template format + hosting |
| Personalised templates | Adapt to user content/style [2] | M/D | P | L | Scene/beat analysis |
| Skills Marketplace (new Oct 2026) | Share/sell styles, workflows [2] | W | n/a | XL | Plugin/skill format |
| Design Studio 2.0 (May 2026) | Infinite canvas, AI design agent (third-party report) | W | P | XL | Separate product, low priority |

## 8. Audio
| Feature | Description | Plat | Tier | Cx | Hint |
|---|---|---|---|---|---|
| Music library | Licensed music; copyright check [8] | D/M/W | F/P | M | Cutweave: CC0/FMA sources |
| SFX library | Free [8] | D/M/W | F | S | Asset pack |
| Voice effects/changer | (unverified detail) | D/M/W | F/P | M | Pitch-shift DSP |
| Noise reduction / audio enhancer | Hum, echo, noise [8] | D/M/W | P | L | RNNoise/DeepFilterNet |
| Voice enhance | (unverified) | D/W | P | L | Enhancement model |
| Vocal isolation | [8] | D | P | L | Demucs |
| Beat detection / sync | (unverified) | M/D | F | M | Onset detection |
| Auto ducking | (unverified) | D/M | F | M | VAD + gain envelope |
| Fades, volume keyframes | (unverified) | D/M/W | F | S | Gain curve |
| Extract audio | (unverified) | D/M/W | F | S | FFmpeg |
| Recording/voiceover | (unverified) | D/M/W | F | S | Audio capture |
| Loudness normalization | [8] | D | F | M | EBU R128 loudnorm |
| AI music generation | [3] | D/W | P | XL | Text-to-music model |
| Copyright check | [8] | D | F | n/a | Needs fingerprint DB; skip |

## 9. AI features
| Feature | Description | Plat | Tier | Cx | Hint |
|---|---|---|---|---|---|
| Auto captions | see 6 | D/M/W | P | L | ASR |
| Background removal / auto cutout | [8] | D/M/W | F/P | XL | Video matting |
| Auto reframe | see 3 | D/M/W | F/P | L | Detection |
| Enhance quality / upscale | Upscale + denoise [12] | D/W | P | XL | Real-ESRGAN, GPU heavy |
| Frame interpolation | [8] | D/M | P | XL | RIFE |
| Retouch / beauty | Skin smooth, teeth, blemish (search result, secondary) | D/M | F/P | L | Face landmarks |
| AI avatars/characters | Lip-sync to script/voice; digital clone [12] | D/M/W | P | XL | TTS + talking-head |
| AI writer / script to video | [12] | W/D/M | P | XL | LLM + stock retrieval |
| Long video to shorts | [12] | W/D | P | XL | ASR + LLM scoring + reframe |
| Filler word / silence removal | [12] | D/M/W | P | M | ASR word timings |
| Video translator / dubbing / lip-sync | Captions translate [12]; dubbing+lip-sync unverified | W | P | XL | ASR+MT+TTS+lip-sync |
| AI image generator | [3] | D/M/W | P credits | XL | Local diffusion optional |
| AI video generation | Seedance 2.5, Sora 2, Veo 3.1 listed [7][12] | D/W/M | P/Ultra credits | XL | API adapters only |
| AI Edit / AI Extend | [7] | D | P/Ultra | XL | Gen-video model |
| EditPilot | Natural-language timeline edits [2][7] | D | P/credits | XL | LLM tool-calling on edit API |
| Generative/Interactive Edit | Select region, prompt change [2] | D/W | P | XL | Video inpainting |
| Agentic creation (pilot) | [2] | D/W/M | pilot | XL | Agent layer |
| AI Lab (mobile) | [2] | M | n/a | L | LLM + auto-edit |
| Relight | mentioned only [12] | D/M | P (unverified) | XL | Depth/normal + relight |
| Eye contact | (unverified) | M/D | P | L | Face model |
| AI movement / stabilize / flicker | [12] | D/M | P | L | Tracking |
| Image: text/people remover, inpaint | [7] | W | P | XL | LaMa |
| AI transparency checker (pilot) | [2] | W | n/a | M | C2PA/watermark |
| ChatGPT plugin | [2] | ChatGPT | n/a | M | API/MCP |
| AI model/product features | (unverified; sibling apps Pippit/Hypic) | W | P | XL | Generative models |

## 10. Export and publishing
| Feature | Description | Plat | Tier | Cx | Hint |
|---|---|---|---|---|---|
| Resolution | 1080p free; 4K Pro; Pro claimed to 8K [3][4]; free 4K claim conflicts [8] | D/M/W | F/P | M | FFmpeg, GPU scaler |
| Frame rate | Up to 60fps [12] | D/M/W | F/P | S | fps conform |
| Codecs/containers | MP4, MOV [12]; web MP4 only [9] | D/M/W | F | S | x264/x265/ProRes |
| HDR export | (unverified) | M/D | P | XL | PQ/HLG tagging |
| Bitrate control | (unverified) | D | F | S | Rate control |
| GIF export | (unverified) | D/M | F | S | palettegen |
| Audio-only export | (unverified) | D/M | F | S | FFmpeg |
| Direct upload | TikTok, YouTube [12] | D/M/W | F | M | OAuth uploads |
| Watermark | On free export of Pro assets [4] | D/M | F | S | Cutweave: none |
| Multi-version export | [7] | D | P | M | Batch render queue |

## 11. Collaboration, cloud, accounts
| Feature | Description | Plat | Tier | Cx | Hint |
|---|---|---|---|---|---|
| Account required | Sync, AI, saved projects [9] | all | F | n/a | Cutweave: none |
| Cloud storage | 100GB-1TB Pro [3][4] | all | P | XL | |
| Team workspaces | [7] | D/W | P | XL | |
| Cross-device continue | [9] | D/M | F | L | |
| Credits | AI usage credits [7] | D/W | P | n/a | |
| Offline | AI needs internet [9] | D | n/a | n/a | Cutweave: local models |

---
## 12. Context motivating an open alternative
1. ToS controversy (June 2025): Terms updated June 12, 2025 (push notice June 6). Coverage quoted a perpetual, worldwide, irrevocable, royalty-free license to user content and to username/image/likeness, applying to content on CapCut servers incl. cloud drafts; backlash was viral [5][13]. CapCut says the User Content section is unchanged, it never claimed ownership, and changes concerned third-party services, "Company Content", Australia/Japan; its page does not mention AI training [5]. Earlier terms reportedly had similar language [13].
2. Paywall creep: 2025 restructure; reports of export blocks and watermark on Pro-marked assets; Pro price from $9.99 to $19.99/mo [4][9]; multi-cam/advanced grading once locked [11]; AI generation credit-metered with Ultra tier [7]. Trustpilot 1.2/5 with billing complaints [9].
3. US availability: PAFACA signed Apr 24, 2024; CapCut/TikTok dark Jan 19, 2025, restored ~Jan 21; TikTok USDS Joint Venture closed Jan 22, 2026 with safeguards said to cover CapCut; reported available on US app stores Aug 2026, no confirmed new ban date [6]. One secondary source claims unavailability for new US downloads as of Jan 21, 2026 (conflicts; unverified).
4. Privacy/regulatory: reported EU GDPR fine on ByteDance (~EUR 530M, 2025; whether it concerns CapCut is unverified); restrictions on government devices in several countries (secondary sources). AI features need cloud + account [9].
5. Direction: 2026 updates are AI-centric (EditPilot, Seedance 2.5, Design Studio 2.0, agentic creation); core timeline largely unchanged [1][2][7].

---
## Sources
1. https://pixflow.net/blog/capcut-2026-update/
2. https://www.capcut.com/newsroom/capcut-world-la (Oct 26, 2026)
3. https://www.capcut.com/resource/capcut-standard-vs-pro
4. https://bigvu.tv/blog/capcut-free-vs-pro-what-2026s-restructure-actually-gives-you/
5. https://capcut.com/resource/about-capcut-terms-of-service
6. https://www.miracamp.com/learn/capcut/ban
7. https://www.capcut.com/resource/seedance-2-5-is-now-on-capcut-pc
8. https://www.capcut.com/resource/pc-professional-video-editor
9. https://filmora.wondershare.com/video-editor-review/capcut-for-pc.html (competitor; treat with caution)
11. https://www.capcut.com/help/capcut-pro-benefits-updates
12. https://www.capcut.com/resource/capcut-ai
13. https://www.digitalcameraworld.com/tech/social-media/capcuts-new-terms-of-service-are-angering-the-internet-and-so-confusing-that-video-editors-are-asking-chatgpt-to-decipher-them
- Search-only: https://www.techradar.com/pro/popular-video-editing-app-capcut-wants-to-use-any-content-you-produce-for-free-forever-heres-what-you-should-know ; https://alternativeto.net/news/2026/5/capcut-unveils-design-studio-2-0-with-infinite-canvas-ai-design-agent-and-pencil-prompting/ ; https://www.capcut.com/tools/keyframe-animation ; https://www.descript.com/blog/article/capcut-ban-what-to-know-and-which-capcut-alternatives-to-consider ; https://www.capcut.com/help/features-needs-to-paid

Limitations: Reddit/YouTube/mobile release notes were not reached this session; mobile-only features and exact free/pro gating per item are largely unverified.
