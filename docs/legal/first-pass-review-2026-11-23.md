# Licensing first-pass review (2026-11-23)

> **Not legal advice.** This is a first-pass review written at the product owner's request by an AI agent working in the repository. It is meant to save counsel time, not to replace counsel. A qualified lawyer must review the plugin exception, the inbound licensing terms and the codec posture before the first external contribution is merged and before the exception is adopted.

- **Scope:** the plugin exception draft, inbound licensing (DCO, CONTRIBUTING), the MIT OR Apache-2.0 boundary, third-party GPL code (FFmpeg), patents, trademark, and anything likely to surprise a contributor or a commercial plugin vendor.
- **Read:** [LICENSE](../../LICENSE), [LICENSE-PLUGIN-EXCEPTION.md](../../LICENSE-PLUGIN-EXCEPTION.md) (draft as of `main` at 7728d7c), [CONTRIBUTING.md](../../CONTRIBUTING.md), [ADR-0011](../adr/0011-licensing.md), [ADR-0010](../adr/0010-layered-plugin-system.md), [product brief](../00-product-brief.md) D4 and D8, [architecture](../01-architecture.md) §2, §3, §12, §14, §15 and §21, [deny.toml](../../deny.toml), [REUSE.toml](../../REUSE.toml), [the FFmpeg bundle README](../../scripts/ffmpeg/README.md), [README.md](../../README.md), [GOVERNANCE.md](../../GOVERNANCE.md), [CODEOWNERS](../../.github/CODEOWNERS), [research 07](../research/07-plugin-architecture.md) §8, and WBS items [WP-0B.01/0B.02](../wbs/P0-foundations.md) and [WP-6A.09](../wbs/P6-platform.md).
- **Changed in the same branch:** the exception draft (rewritten, still DRAFT), the CONTRIBUTING DCO and licensing sections, and the README license paragraph. See [Changes applied](#changes-applied).

## Summary

The overall design is sound and has good precedent: a GPL-3.0-or-later host, a permissive interface layer (WIT, SDK, composition runtime), a written GPLv3 §7 additional permission, a DCO, and no native code in-process. The closest analogue is the Qt GPL Exception 1.0 used for GPL-3.0 Qt applications; GIMP's LGPL `libgimp` plus out-of-process plug-ins shows the same separation working in practice.

The draft exception needed more than polish. Its operative sentence licensed the plugin, but did not clearly permit what plugin vendors actually need: combining plugins with Cutweave, shipping them together, and keeping plugin source out of Cutweave's Corresponding Source. Its definition of "Cutweave" reached code the project cannot license. The inbound paragraph did not attach the exception to contributions reliably. These are the three blockers. The rest is scope (which interfaces count), GPL-licensed interface material that vendors will naturally copy, patents, trademark, and a few public statements that say more than is true today.

| ID | Severity | Finding | Status |
|---|---|---|---|
| [B1](#b1-inbound-licensing-does-not-yet-carry-the-exception) | blocker | Inbound licensing does not yet carry the exception | CONTRIBUTING edited; adoption gate and headers pending counsel |
| [B2](#b2-the-operative-grant-does-not-permit-what-plugin-vendors-need) | blocker | The operative grant does not permit what plugin vendors need | Applied (exception §1-§2), flagged for counsel |
| [B3](#b3-scope-of-the-grant-versus-third-party-gpl-code-ffmpeg) | blocker | Scope of the grant versus third-party GPL code (FFmpeg) | Text applied (§0, §4.2); architectural position needs a decision |
| [S1](#s1-the-removal-clause-misstates-gplv3-7) | should-fix | The removal clause misstates GPLv3 §7 | Applied (§6) |
| [S2](#s2-versioning-or-later-grantor-and-the-notice-mechanism) | should-fix | Versioning, "or later", grantor and the notice mechanism | Applied (title, §0, §1, adoption steps); tooling to verify |
| [S3](#s3-the-published-interface-list-is-incomplete-and-uses-wrong-names) | should-fix | The Published Interface list is incomplete and uses wrong names | Applied (§0) |
| [S4](#s4-gpl-licensed-interface-material-schema-generated-types-docs) | should-fix | GPL-licensed interface material (`schema/`, generated types, docs) | Applied in exception; ADR follow-up proposed |
| [S5](#s5-no-inference-clause-for-arms-length-clients) | should-fix | No-inference clause for arm's-length clients | Applied (§5) |
| [S6](#s6-conditions-on-plugins-and-corresponding-source) | should-fix | Conditions on plugins and Corresponding Source | Applied (§1.4, §2) |
| [S7](#s7-first-party-content-and-output) | should-fix | First-party content and output | Output permission applied (§3); content licensing ADR proposed |
| [S8](#s8-patents) | should-fix | Patents | Applied (§4.5); questions for counsel |
| [S9](#s9-trademark) | should-fix | Trademark | Disclaimer kept; policy proposed |
| [S10](#s10-the-mit-or-apache-20-boundary) | should-fix | The MIT OR Apache-2.0 boundary | CONTRIBUTING applied; CI check proposed |
| [S11](#s11-public-statements-say-the-exception-is-in-effect) | should-fix | Public statements say the exception is in effect | README applied; ADR-0011 change proposed |
| [S12](#s12-ai-authored-code) | should-fix | AI-authored code | Question for counsel |
| [S13](#s13-gpl-compliance-for-cutweaves-own-binaries) | should-fix | GPL compliance for Cutweave's own binaries | Proposed (7F.01, 7F.02) |
| [S14](#s14-decisions-the-product-owner-should-make-knowingly) | should-fix | Decisions the product owner should make knowingly | Decision needed |
| [N1](#n1-license-has-one-extra-trailing-newline) to [N7](#n7-dco-identity) | nit | Seven small items | Partly applied |

Severity: **blocker** = must be fixed before the exception is adopted or before the first external contribution is merged; **should-fix** = fix before adoption or before third-party plugin distribution (P6); **nit** = wording, consistency or hygiene.

## Blockers

### B1. Inbound licensing does not yet carry the exception

**Where:** [CONTRIBUTING.md](../../CONTRIBUTING.md) "Sign-off (DCO)" and "Licensing"; SPDX headers and [REUSE.toml](../../REUSE.toml); [ADR-0011](../adr/0011-licensing.md).

**Text at issue (before this branch):**

> Every commit must carry a `Signed-off-by:` line, which certifies that you wrote the change or have the right to submit it under the project's license

> By submitting a contribution, you agree that it is licensed under GPL-3.0-or-later and that Cutweave may also distribute it under the additional permission in LICENSE-PLUGIN-EXCEPTION.md once that text is adopted. If you do not agree, say so in your pull request before it is merged.

**Why.** An additional permission covers only material whose copyright holders grant it (GPLv3 §7 ¶2: you may place additional permissions on material "for which you have or can give appropriate copyright permission"; FSF FAQ [LinkingOverControlledInterface](https://www.gnu.org/licenses/gpl-faq.html#LinkingOverControlledInterface): only the copyright holders can authorize such an exception). Four gaps follow:

1. **The DCO certifies the license "indicated in the file".** DCO 1.1 (a) and (b) refer to "the open source license indicated in the file". Every file today says `GPL-3.0-or-later` with no exception, and REUSE.toml says the same for unheadered files. A sign-off therefore certifies a GPL-only contribution. GitHub's Terms of Service §D.6 ("Contributions Under Repository License", inbound = outbound) points the same way: it licenses contributions under the repository's license notice, which is GPL alone. The CONTRIBUTING paragraph is the only thing that adds the exception, and a statement in a Markdown file that contributors may never read is weak evidence of consent.
2. **The opt-out creates permanent holes.** "If you do not agree, say so" invites contributions that are GPL-only forever. Each one shrinks the code the exception covers, and nobody can later tell a plugin vendor which files those are.
3. **Consent to an unseen text.** "once that text is adopted" asks contributors to agree to whatever text is adopted later. Counsel should say whether that is effective; it is safer to bound it (the exception may only add permissions for plugins that use published interfaces).
4. **ADR-0011 assumed the exception ships from day one.** It says "a DCO cannot add exceptions later, so the exception ships from day one". It has not shipped; it is a draft not in effect. The plan only works if adoption happens before any external contribution is merged.

Today the gap is closable cheaply: all 236 non-merge commits on `main` have one human author and all carry a `Signed-off-by` (most are co-authored by AI models; see S12). The existing copyright holder can adopt the exception unilaterally, provided counsel confirms who that holder is (the commits use an organisational e-mail address, so the holder may be an organisation rather than the individual).

**Proposed text (applied to CONTRIBUTING.md, still marked pending legal review):**

> **Your sign-off and the license in the file.** Your `Signed-off-by:` certifies, under the DCO, that you may submit your change under the license indicated in each file you change, which is its SPDX header (or `REUSE.toml` for files without one). Do not change a file's license header unless a maintainer asks you to.
>
> **The plugin exception (pending legal review).** An additional permission such as the plugin exception covers only code whose authors agreed to it. By signing off a contribution to a GPL-licensed part of Cutweave, you also agree that it may be distributed under GPL-3.0-or-later together with the plugin exception in LICENSE-PLUGIN-EXCEPTION.md, as adopted by the maintainers after legal review. Any adopted version may only add permissions for works that use Cutweave's published interfaces; it never removes rights the GPL gives. The maintainers do not accept contributions to GPL-licensed parts of Cutweave on other terms, and will replace this paragraph with counsel-reviewed wording, and adopt the exception, before the first external contribution is merged.

**Still to do (not applied; for counsel and the maintainers):**

- Adopt the exception before the first external merge, and change the file notices to name it (exception "Applying the exception", step 2), so the DCO and GitHub §D.6 carry it.
- Add a line to the PR template's contribution checklist: "I agree to the licensing terms in CONTRIBUTING.md, including the plugin exception." This gives a per-PR record of consent at no cost.
- Amend ADR-0011 (new ADR, Proposed) to record that the exception ships before the first external contribution rather than "from day one", and that the DCO-only versus "CLA-lite" question in WP-6A.09 is settled by adopting first.
- A CLA is only needed if the project wants to change the exception later in ways the bounded consent does not cover, or to relicense. ADR-0011 rejected a CLA; this review does not reopen that.

### B2. The operative grant does not permit what plugin vendors need

**Where:** exception §3 (before this branch).

**Text at issue:**

> Under GPL-3.0 section 7, you are granted the following additional permission.
>
> You may create, convey and license a Plugin under terms of your choice, including terms that are not compatible with the GPL, and including proprietary terms, provided that the Plugin interacts with Cutweave only through Published Interfaces.

**Why.**

1. **It permits the wrong act.** The GPL does not restrict a separate work at all; it restricts covered works. Where a plugin and Cutweave form one combined program (FSF FAQ [GPLPlugins](https://www.gnu.org/licenses/gpl-faq.html#GPLPlugins): dynamically linked plug-ins that call each other and share data structures form a single combined program), the GPL condition that bites is §5(c): "You must license the entire work, as a whole, under this License". The permission therefore has to be about the combination and its conveyance. The Classpath exception does this ("permission to link this library with independent modules to produce an executable … and to copy and distribute the resulting executable under terms of your choice"); so does Qt GPL Exception 1.0 (combining the application with plugins under terms of your choice and distributing the resulting executable); so does GCC RLE 3.1 §1 (propagating Target Code "formed by combining the Runtime Library with Independent Modules").
2. **It does not let anyone ship plugins with Cutweave.** GPLv3 §5's aggregate safe harbour applies only to separate works "which are not by their nature extensions of the covered work". A plugin is by its nature an extension, so the safe harbour arguably does not cover an installer, a distro package or a plugin index that bundles proprietary plugins with Cutweave. Bundles are a normal commercial case and need an explicit permission.
3. **Corresponding Source is not addressed.** GPLv3 §1 counts as Corresponding Source "dynamically linked subprograms that the work is specifically designed to require". A plugin shipped with Cutweave and wired into it could be argued to fall within that. The exception should say plugin source is not part of Cutweave's Corresponding Source.
4. **The grantor is unnamed.** "you are granted" does not say by whom. An additional permission is a grant by copyright holders; the text should say so (see B3 for which holders).

**Proposed text (applied as exception §1, "Grant of additional permissions"):**

> The copyright holders of Cutweave Code grant you the following additional permissions under section 7 of the GPL. […]
>
> 1. **Combination.** You may combine Cutweave Code with Plugins, whether Cutweave loads, links, executes, sandboxes or invokes a Plugin at run time or the Plugin controls Cutweave from another process, and you may convey the result. The GPL does not require the Plugins, or the combination as a whole, to be licensed under the GPL; the Cutweave Code in the combination remains under the GPL.
> 2. **Plugin terms.** You may license and convey a Plugin, including any Interface Material it contains, under terms of your choice, including proprietary terms.
> 3. **Distribution together.** You may convey a Plugin together with Cutweave, for example in the same installer, archive, package, plugin index or device, provided that you convey Cutweave itself under the GPL, with or without this exception.
> 4. **Corresponding Source.** The Corresponding Source of Cutweave, as the GPL defines it, does not include the source code of a Plugin.

Counsel should confirm that this is effective in the jurisdictions where the maintainers and the main distributors are based, and whether Qt GPL Exception 1.0's condition (a prominent offer of the application's source accompanying the combined executable) should be added. This review did not add it, because the GPL already requires the source offer for Cutweave itself.

### B3. Scope of the grant versus third-party GPL code (FFmpeg)

**Where:** exception §2 "Cutweave" definition and §4.3 (before this branch); [architecture §21](../01-architecture.md); [the FFmpeg bundle README](../../scripts/ffmpeg/README.md); [deny.toml](../../deny.toml).

**Text at issue:**

> **Cutweave** means the program and source files in this repository that are licensed under the GPL, other than the Published Interfaces listed below when they carry their own (MIT OR Apache-2.0) license.

> It does not grant any right to Cutweave's names or logos, and does not affect the licenses of third-party components bundled with Cutweave (including FFmpeg) or of models and assets a Plugin downloads or ships.

> Components under GPL are **not** traps for the host. (architecture §21)

**Why.**

1. **The definition reaches code the project cannot license.** "Files in this repository that are licensed under the GPL" includes any GPL code by third parties that is ever vendored into the repository, and the definition is the object of the grant. The project can grant additional permissions only for material it holds rights in (§7 ¶2; FSF FAQ LinkingOverControlledInterface: you cannot authorize the exception for other authors' GPL code without their approval).
2. **The shipped program contains GPL code whose authors grant nothing.** The engine links the FFmpeg bundle in-process, and that bundle is "GPL-3.0-or-later as a whole" because x264, x265 and vid.stab are GPL-2.0-or-later. WASM components (L2), compositions (L1.5, QuickJS) and WGSL effects (L1) also run inside the engine process (architecture §2, §15). GPLv3 §7 ¶1 says: "If additional permissions apply only to part of the Program, that part may be used separately under those permissions, but the entire Program remains governed by this License without regard to the additional permissions." So the exception protects a plugin vendor against claims by Cutweave's copyright holders, and against nobody else. Whether an in-process proprietary plugin forms one combined work with x264 or x265 is the open question. There are good arguments that it does not: the plugin never calls or links FFmpeg, it talks only to Cutweave's WIT, QuickJS or WGSL interfaces, the canonical ABI copies values across a sandbox boundary, and FFmpeg sits behind `MediaBackend`. But there are no court decisions on this, the FSF's combined-program test is about the program as a whole, and the x264/x265 holders also sell commercial licenses, so they have a commercial reason to enforce.
3. **The dependency policy treats GPL dependencies as harmless.** That is right for the host's own license and wrong for the exception. `deny.toml` admits `GPL-2.0-or-later`, `GPL-3.0-only` and `GPL-3.0-or-later` crates; any such crate linked into the engine adds another set of copyright holders who never granted the exception.

**Proposed text (applied as exception §0 "Cutweave Code" and §4.2):**

> **Cutweave Code** means material in Cutweave whose copyright holders license it under the GPL together with this exception, as stated in the material's license notice or in a notice that covers it. Cutweave Code does not include material under other licenses, such as the MIT OR Apache-2.0 parts of the Published Interfaces, or third-party components distributed with Cutweave, such as FFmpeg and the libraries linked into it.

> This exception is granted only by the copyright holders of Cutweave Code. It grants no permission for third-party components distributed with or used by Cutweave, including FFmpeg and the libraries linked into it, some of which are licensed under the GPL. Those components remain under their own licenses, and section 7 of the GPL provides that where additional permissions apply to only part of a program, the program as a whole remains governed by the GPL without regard to them. This exception does not decide whether combining a Plugin with a Cutweave build that includes such components needs permission from their copyright holders.

**Decision needed before adoption** (this is why it is a blocker; the text above only states the limit honestly):

- **Option A, document the limit.** Keep the architecture and tell vendors plainly (in the exception and in `docs/legal/plugin-exception.md`, WP-6A.09) that in-process plugins rely on the separate-works argument as far as third-party GPL components are concerned. Cheapest; leaves vendor risk where it is.
- **Option B, keep GPL-only third-party code out of the plugin host process.** Run FFmpeg decode and encode in a separate process (the `cutweave worker` mode already exists, architecture §2), or host WASM plugins in a separate process. Strongest separation; costs performance and engineering (ADR-0003 is still Proposed, so this is the cheapest moment to decide).
- **Option C, offer a build without GPL-only codecs.** An LGPL FFmpeg configuration (no x264, x265 or vid.stab; OS and hardware encoders only, which §21 already prefers for patent reasons) for distributions that bundle proprietary plugins. Removes the third-party GPL holders from the question for those builds.

Whatever is chosen, add to §21 and `deny.toml`: any non-LGPL GPL component linked into the engine process needs integrator approval and an entry in a register of third-party GPL components, because each one narrows what the plugin exception can promise. Also replace "Components under GPL are **not** traps for the host" with a sentence that keeps the host point and adds this one.

## Should-fix

### S1. The removal clause misstates GPLv3 §7

**Where:** exception §5 (before this branch).

**Text at issue:**

> As permitted by GPL-3.0 section 7, a person who conveys a modified version of Cutweave may, at their option, remove this additional permission from their version, or from any part of it. The permission is retained for a modified version unless it is explicitly removed. If you remove it, you must say so in a prominent notice.

**Why.** GPLv3 §7 ¶2 lets anyone who conveys "a copy of a covered work", modified or not, remove additional permissions "from that copy, or from any part of it". The draft narrowed that to modified versions. It also made a prominent notice mandatory on removal; once the permission is removed, its terms no longer bind, and a condition on removing an additional permission looks like a "further restriction" on a right the GPL gives (§7 ¶¶ on further restrictions, §10). Precedent: the Classpath exception simply says that if you do not wish to extend it, delete it.

**Proposed text (applied as exception §6):**

> When you convey a copy of Cutweave, modified or not, you may remove this exception from that copy or from any part of it, as section 7 of the GPL permits. If you do not remove it, it continues to apply. You may also extend it to material you add to Cutweave, if you have or can give the copyright permission to do so. If you remove it, please say so prominently, so that plugin authors are not misled.

### S2. Versioning, "or later", grantor and the notice mechanism

**Where:** exception title and §1, §3 (before this branch); SPDX headers; reviewer note on the identifier.

**Text at issue:**

> # Cutweave Plugin Exception

> Confirm the final mechanism: a SPDX-style `GPL-3.0-or-later WITH Cutweave-plugin-exception` identifier, […]

**Why.**

1. **No version.** An exception that may need fixing later must be identifiable. GCC's is "3.1", Classpath's "2.0", Qt's "1.0".
2. **"Or later".** Cutweave is GPL-3.0-or-later; the draft spoke only of "GPL-3.0 section 7". A recipient who elects a later GPL (§14) should keep the permissions, to the extent that version allows them.
3. **Notice.** GPLv3 §7, last paragraphs: if you add terms "you must place, in the relevant source files, a statement of the additional terms that apply to those files, or a notice indicating where to find the applicable terms". The per-file SPDX header is the natural place, and it is also what the DCO certifies (B1).
4. **Identifier.** `Cutweave-plugin-exception` is not valid SPDX: after `WITH`, SPDX 2.x allows only exceptions on the SPDX list. SPDX 3.0 adds user-defined additions with the `AdditionRef-` prefix (e.g. `GPL-3.0-or-later WITH AdditionRef-…`). Tool support varies (REUSE, cargo-deny's `spdx` crate, npm's `license` validation); check before switching. The long-term route is to ask SPDX to list the exception once it is final.

**Proposed text (applied):** title "Cutweave Plugin Exception, version 1.0"; §0 "**GPL** means the GNU General Public License, version 3, or any later version published by the Free Software Foundation that you choose to apply under its section 14"; §1 "If you apply a later version of the GPL, they apply as additional permissions under that version to the extent it allows them"; and an "Applying the exception" section with `GPL-3.0-or-later WITH AdditionRef-Cutweave-plugin-exception-1.0` (to be confirmed) and a fallback of a one-line notice pointing to the exception file.

### S3. The Published Interface list is incomplete and uses wrong names

**Where:** exception §2 "Published Interface" (before this branch).

**Text at issue:**

> 2. the `@cutweave/sdk` package;
> 3. the `@cutweave/compose` package and its composition runtime API;

**Why.**

- **Wrong package names.** The packages are `@cutweave/plugin-sdk` and `@cutweave/compose-runtime` ([plugin-sdk package.json](../../packages/plugin-sdk/package.json), [compose-runtime package.json](../../packages/compose-runtime/package.json)). Architecture §3 and §12 and ADR-0010 still say `@cutweave/sdk` and `@cutweave/compose`. A license that names a package that does not exist is the kind of mismatch a vendor's lawyer will flag. Name the directories, with "whatever package names they are published under".
- **Missing interfaces.**
  - L3 UI extensions: the contribution points and message protocol (ADR-0011 lists "iframe RPC"; architecture §15 now uses separate `WebContentsView` processes).
  - The project file format (ADR-0006). It is a contract, and converters and asset managers will read it.
  - L4: the out-of-process native host protocol, and the third-party standard interfaces it hosts (CLAP, VST3, OpenFX). Such plugins are written against those standards, not against Cutweave, and should be named so nobody wonders.
  - Generated glue: WGSL binding code the host generates for effects (research 07 §2: the host generates the uniform struct), and bindings generated from WIT.
- **Growth.** New interface categories will appear (architecture §14b's Agent Kit, publishers). Without a designation mechanism, each would need a new exception version, which with a DCO-only project means re-collecting consent.
- **Forks.** The draft's "in the form published in a Cutweave release" did not say whose release. A fork could add an "interface" that exposes Internals. The FSF's controlled-interface template handles this by withdrawing the exception from versions that modify the interface; here the interfaces are permissive, so it is simpler to say only interfaces released by the Cutweave Project count.

**Proposed text (applied):** see the nine-item list in exception §0, ending with "any other interface that the Cutweave Project designates as a Published Interface in the documentation of a release" and "An interface added or changed by anyone other than the Cutweave Project is not a Published Interface." Counsel should confirm that item 9 is covered by contributors' consent (B1).

**Follow-up (not applied):** align architecture §3, §12 and §15 and ADR-0010/0011 on package names and on `WebContentsView` versus iframes (N5).

### S4. GPL-licensed interface material (`schema/`, generated types, docs)

**Where:** [REUSE.toml](../../REUSE.toml) (everything outside the three permissive directories is GPL); `schema/` (`commands.json`, `queries.json`, `events.json`, `mcp-tools.json`); [`packages/protocol`](../../packages/protocol/package.json) (`GPL-3.0-or-later`, generated types plus typed RPC client); `docs/`; `packages/tokens`.

**Text at issue (draft §3, second paragraph):**

> […] and the case where a Plugin includes or links the code of a Published Interface that is itself licensed under MIT OR Apache-2.0 (for example the SDK).

**Why.** The machine-readable contract that an MCP or JSON-RPC client, a code generator or a UI extension would copy is GPL today. A vendor who generates client types from `schema/commands.json`, vendors `packages/protocol/src/generated/`, pastes a documented example, or copies design tokens into a panel has copied GPL material, and the draft's permission covered only the MIT OR Apache-2.0 parts. Whether API schemas are copyrightable at all is unsettled (Google v. Oracle, 2021, was decided on fair use without resolving copyrightability), but generated schemas here carry human-written documentation strings, so assume they are.

**Proposed text (applied):** exception §0 defines **Interface Material** ("the parts of a Published Interface that are Cutweave Code, such as schemas, generated type definitions and bindings, generated binding code and documented examples") and §1.2 lets a Plugin include it under any terms. Item 6 of the Published Interface list names `schema/` and `packages/protocol/src/generated/`. The hand-written client in `packages/protocol` stays GPL.

**Follow-up (not applied; needs an ADR, since ADR-0011 fixes the permissive set):** license `schema/`, the generated TS types and a small client package under MIT OR Apache-2.0, and put documentation code samples under a permissive license or CC0. That is cleaner for vendors than relying on the exception, and it survives a fork that removes the exception. The inbound terms (B1) already let the project do this for future contributions only if contributors agreed to it; counsel should say whether the exception's Interface Material permission is enough or whether CONTRIBUTING should say so explicitly.

### S5. No-inference clause for arm's-length clients

**Where:** exception §2 "Plugin" (before this branch) included "an external program or agent that controls Cutweave through MCP, the CLI or JSON-RPC".

**Why.** Listing MCP and JSON-RPC clients as needing a permission can be read as an admission that they would otherwise be derivative works. Under the FSF's own analysis, programs that communicate at arm's length over pipes, sockets or command lines are normally separate works (FAQ [GPLPlugins](https://www.gnu.org/licenses/gpl-faq.html#GPLPlugins), [MereAggregation](https://www.gnu.org/licenses/gpl-faq.html#MereAggregation)). GCC RLE 3.1 §2 ("No Weakening of GCC Copyleft") and the Linux kernel's `Linux-syscall-note` show the two standard moves: say the exception creates no presumption, and clarify rather than grant for the arm's-length case.

**Proposed text (applied as exception §5):**

> This exception does not imply that a work that communicates with Cutweave at arm's length, for example over MCP, the CLI or a network protocol, would otherwise be a work based on Cutweave, nor does it imply that any work is not. Nothing in this exception weakens the GPL's requirements for Cutweave itself.

### S6. Conditions on plugins and Corresponding Source

**Where:** exception §3-§4 (before this branch).

**Why.** Three things were missing.

1. **No restriction on Cutweave rights.** A bundle could ship a proprietary plugin under an EULA that forbids modifying or reverse-engineering "the software", Cutweave included. §10 already forbids further restrictions on Cutweave, but the exception should not be available to a bundle that tries. The Classpath exception's equivalent is "provided that you also meet, for each linked independent module, the terms and conditions of the license of that module".
2. **Plugin source** (B2 point 3).
3. **What a Plugin may contain.** The draft did not say a Plugin may contain no Cutweave code other than interface material, so a "plugin" that copied an internal crate could argue it still "interacts only through Published Interfaces".

**Proposed text (applied as exception §0 "Plugin" (b), §1.4 and §2):**

> The permissions in section 1 apply to a Plugin only while: 1. the Plugin meets the definition of Plugin in section 0; and 2. the terms under which you convey the Plugin, or Cutweave together with it, do not restrict any recipient's exercise of the rights the GPL grants for Cutweave. You must still meet every condition of the GPL for the Cutweave Code you convey.

### S7. First-party content and output

**Where:** [REUSE.toml](../../REUSE.toml) default; architecture §3 (`plugins/`, `agent-kit/`, `catalog/`), §12 ("Built-in text animations, caption styles and many templates are themselves compositions"), §14b (Agent Kit skills and packs), §15 (L0 packs).

**Why.** Two separate surprises.

1. **Output that contains GPL assets.** GPLv3 §2 covers output "only if the output, given its content, constitutes a covered work". A rendered video that contains a built-in sticker, sound effect, LUT-graded look, title animation or font is output whose content includes GPL-licensed material. The FSF says output is covered when the program emits its own art or audio (FAQ [WhatCaseIsOutputGPL](https://www.gnu.org/licenses/gpl-faq.html#WhatCaseIsOutputGPL)), which is why the GPL font exception exists (FAQ FontException) and why Qt GPL Exception 1.0 and the Autoconf exception have output permissions. Users of a video editor will not expect their videos to carry GPL obligations.
2. **Starting from first-party work makes your work GPL.** First-party plugins (`plugins/`), built-in compositions and templates, preset and prompt packs and the Agent Kit skills are all GPL by default. A vendor who starts a commercial template from a built-in one, or a user who adapts a skill into another agent's configuration, takes on the GPL. Vendors will copy first-party examples; that is what examples are for.

**Proposed text (applied as exception §3, flagged for counsel):**

> Rendered video, audio, still images, captions and similar media produced by running Cutweave are not made subject to the GPL merely because they contain copies of Cutweave Code that Cutweave places in its output, such as built-in templates, effects, presets, fonts or other assets. You may use and convey such output under terms of your choice. This section does not apply to source code (for example a composition, effect or template copied or adapted from Cutweave Code), and it does not affect any other reason why output might be subject to the GPL or another license.

**Follow-up (not applied; needs an ADR amending ADR-0011):** license first-party content (stickers, SFX, music, LUTs, caption styles, templates, compositions meant as starting points, Agent Kit skills and packs) under CC0-1.0 or CC-BY-4.0, and plugin scaffolding and examples under MIT OR Apache-2.0. With that in place, exception §3 becomes a backstop. Architecture §21's allow list also lacks a font license; bundled fonts are typically OFL-1.1 (Inter, used in §12's example, is), which is fine as separately licensed data but should be listed.

### S8. Patents

**Where:** exception (silent before this branch); architecture §21 (codec patents); FFmpeg bundle (x264, x265; SVT-AV1 under BSD-3-Clause-Clear plus the AOMedia Patent License 1.0).

**Why and analysis.**

- **GPLv3 §11** gives each contributor's patent license for "the contents of its contributor version". It covers Cutweave as contributed, not a plugin's own functionality. A plugin vendor who never conveys Cutweave does not need the GPL at all (§9) and is not bound by §10's ban on patent suits; a vendor who ships Cutweave in a bundle (B2's §1.3) is a GPL licensee for Cutweave and is bound.
- **Apache-2.0 §3** on the SDK, WIT and compose runtime gives a contributor patent license with a termination-on-suit clause, but only to a recipient who chooses Apache-2.0. A recipient who chooses MIT gets no express patent license. That is normal for the Rust ecosystem; vendors who care about patents should elect Apache-2.0. Worth one sentence in the plugin-author docs.
- **Codec plugins and bundles.** GPLv3 §11 ¶¶ 5-7 and §12 bite on anyone who conveys Cutweave while relying on, or granting, a patent license. A vendor who ships "Cutweave plus an HEVC encoder plugin" and buys a codec patent license scoped to that bundle may find the license extended to all recipients of Cutweave (¶ 6), or may be unable to convey Cutweave at all if the license is discriminatory (¶ 7, §12). Vendors should scope patent licenses to the plugin, or ship the plugin separately. This belongs in the vendor FAQ.
- **The project's own posture.** Shipping x264 and x265 encoders (H.264, HEVC) in every build is the main patent exposure; it is tracked for 7F.03 and §21 already says to seek review before 1.0. Counsel should also confirm that SVT-AV1's AOMedia Patent License 1.0 (a third-party patent license with a defensive termination) raises no §10-§12 issue for the GPL bundle.
- **Patent peace in the exception?** The exception could withdraw its permissions from anyone who sues alleging that Cutweave infringes a patent. GCC RLE 3.1 and Classpath do not do this; Apache-2.0 does. It would protect users, but it adds a condition vendors must review. Left as a question for counsel.

**Proposed text (applied as exception §4.5):**

> It grants no patent license. The patent licenses for Cutweave are those in section 11 of the GPL and, for parts licensed under MIT OR Apache-2.0 when you choose Apache-2.0, those in section 3 of Apache-2.0.

### S9. Trademark

**Where:** exception §4.3 (before this branch) and §4.4 (now); no trademark policy exists (WP-0B.01 marks it out of scope).

**Why.** Neither license grants trademark rights: Apache-2.0 §6 says so expressly, MIT is silent, and GPLv3 §7(e) lets a licensor add a term "declining to grant rights under trademark law". Nothing stops a fork or a plugin vendor from using "Cutweave" in a confusing way except trademark law, and the project has no registration or policy yet. Plugin vendors need to know they may say "for Cutweave"; forks need to know whether they must rename.

**Proposed (not applied):**

- A `TRADEMARKS.md` before third-party plugin distribution (P6): nominative use allowed ("X for Cutweave", "works with Cutweave"); forks that change functionality and redistribute must use a different name, while unmodified rebuilds and distro packages may keep it; plugin index naming rules; logo use; no implied endorsement.
- Clearance search and registration in the main markets before 1.0 (question for counsel).
- Reserve the `@cutweave` npm scope and `cutweave-*` crate names on crates.io before the SDK is published, to avoid squatting.
- Keep the trademark disclaimer in the exception (§4.4, applied), and add one line to CONTRIBUTING (applied).

### S10. The MIT OR Apache-2.0 boundary

**Where:** [CONTRIBUTING.md](../../CONTRIBUTING.md) "Licensing"; [ADR-0011](../adr/0011-licensing.md) consequences; [xtask headers](../../xtask/src/headers.rs); [CODEOWNERS](../../.github/CODEOWNERS).

**Text at issue:**

> The plugin SDK, WIT interfaces and composition runtime (`packages/plugin-sdk`, `wit/`, `packages/compose-runtime`) are intended to be **MIT OR Apache-2.0**, and contributions to those directories are made under both licenses.

**Why.**

- "Intended to be" casts doubt on files that already carry `MIT OR Apache-2.0` headers and license files.
- **Inbound for the permissive part.** The DCO plus the file header covers it, and Apache-2.0 §5 adds that contributions "intentionally submitted for inclusion in the Work" are under Apache-2.0 unless stated otherwise. MIT has no inbound clause. The Rust ecosystem's standard sentence covers both and should be used verbatim.
- **Do SDK headers and WIT files make plugins derivative works?** A plugin built with the SDK or generated from WIT contains MIT OR Apache-2.0 code, which any license may include if the notices are kept. So the permissive layer does its job, *provided it stays permissive*: no GPL code may flow into it. ADR-0011 says so, but nothing enforces it. A permissive package that imported `@cutweave/protocol` or `@cutweave/tokens` (both GPL), or contained code generated from GPL sources, would quietly make every plugin that uses it GPL-bound. The SFLC's guide "Maintaining Permissive-Licensed Files in a GPL-Licensed Project" (2007) describes the same one-way flow.
- **Review coverage.** CODEOWNERS and GOVERNANCE route `wit/` and `packages/plugin-sdk` to legal review but not `packages/compose-runtime` (N2).

**Proposed text (applied to CONTRIBUTING.md):**

> Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in `wit/`, `packages/plugin-sdk` or `packages/compose-runtime`, as defined in the Apache-2.0 license, is licensed as above (MIT OR Apache-2.0), without any additional terms or conditions.
>
> Code may move from those directories into GPL-licensed parts of Cutweave (keep its notices), but not the other way: do not copy GPL-licensed code, or code generated from it, into them unless every author of that code agrees in writing.

**Follow-up (not applied):** extend `cargo xtask check-deps` (or the JS license check) so the three permissive packages may depend only on permissively licensed packages and on no GPL workspace package; add `packages/compose-runtime/`, `REUSE.toml` and `schema/` to the legal-review CODEOWNERS entries.

### S11. Public statements say the exception is in effect

**Where:** [README.md](../../README.md) "License"; [ADR-0011](../adr/0011-licensing.md); [product brief](../00-product-brief.md) D4.

**Text at issue:**

> Cutweave is GPL-3.0, with a plugin exception: plugins that use only the published interfaces may use any license. (README)

> Research 07 notes that a DCO cannot add exceptions later, so the exception ships from day one and relicensing is not planned. (ADR-0011)

**Why.** The README tells a reader, today, that proprietary plugins are permitted. That is false until adoption, and a vendor who relied on it could argue estoppel or implied license against the project, which is the opposite of the careful posture in the exception's own banner. "GPL-3.0" also omits "or later", which matters for the exception's "later version" clause (S2).

**Proposed text (applied to README.md):**

> Cutweave is licensed under GPL-3.0-or-later. The plugin SDK, WIT interfaces and composition runtime are MIT OR Apache-2.0. A plugin exception that would let plugins using only the published interfaces use any license is being drafted; it is pending legal review and not yet in effect (see LICENSE-PLUGIN-EXCEPTION.md, linked in the README). Contributions require a DCO sign-off.

**Follow-up (not applied):** a new Proposed ADR amending ADR-0011 on timing (B1), on the permissive set (S4, S7) and on the dependency register (B3); product brief D4 could say "GPL-3.0-or-later".

### S12. AI-authored code

**Where:** [CONTRIBUTING.md](../../CONTRIBUTING.md) "AI-assisted and AI-agent contributions"; commit history (nearly every commit carries a `Co-Authored-By` trailer naming an AI model).

**Why.** Two questions for counsel, neither of which blocks the exception.

1. **Copyrightability.** The US Copyright Office's report *Copyright and Artificial Intelligence, Part 2: Copyrightability* (29 January 2025) concludes, in summary, that material generated entirely by AI is not protected, and that prompts alone are generally not enough, while human-authored expression, creative selection and arrangement, and human modifications are protected. Other jurisdictions differ. If much of Cutweave is not protected, the GPL (and the exception) has less to enforce in those parts, and a closed fork is harder to stop. This does not make the licensing wrong; it changes how strong the copyleft is.
2. **The DCO and agent output.** DCO (a) says the contribution "was created in whole or in part by me". For agent output, the human operator's certification rests on review and direction rather than authorship. The CONTRIBUTING rules (disclosure, human accountability, no tainted code) are the right shape; counsel should confirm the wording.

**Proposed (not applied; outside the paragraph this review was asked to edit):** after counsel's answer, add to the AI section that by signing off an AI-assisted contribution the operator certifies (b) or (a) on the basis of having reviewed and directed it, and that they are not aware of the output reproducing third-party code.

### S13. GPL compliance for Cutweave's own binaries

**Where:** architecture §21; [the FFmpeg bundle README](../../scripts/ffmpeg/README.md) "Licensing obligations"; WBS 7F.01 and 7F.02.

**Why.** The FFmpeg bundle's compliance plan is careful (complete source archive from the same CI run, configure line, license texts, no `--enable-nonfree`). Two things for Cutweave itself are not written down yet:

- **Appropriate Legal Notices.** GPLv3 §5(d) requires each interactive user interface to display Appropriate Legal Notices (defined in §0: a copyright notice, that there is no warranty, that the work may be conveyed under the GPL, and how to view a copy of the GPL). The desktop app needs an About or Legal screen, and the interactive CLI or MCP modes should have a `--license` style output. Add this to 7F.01's acceptance criteria.
- **Corresponding Source for Cutweave builds**, not only FFmpeg: the tagged source, the lockfiles and the build scripts (§1 includes "scripts to control" building and installing), kept available for as long as binaries are offered (§6). Releases should attach or link a source archive for the exact tag, and the third-party notices (cargo-about and the JS collector) should ship inside the app.

### S14. Decisions the product owner should make knowingly

These are not defects; they follow from the exception's design. They should be decided, not discovered.

1. **Closed front ends.** A proprietary editor UI that drives the engine over JSON-RPC or MCP is a Plugin, and §1.3 lets it ship in one installer with Cutweave. Such a front end was probably permitted anyway as an arm's-length program (S5), but the exception now says so. If that is unwanted, §1.3 could exclude "a program that replaces Cutweave's own user interface"; an exception may narrow its own permissions, though it can never take away what the GPL allows.
2. **Open core.** A distributor may move features into proprietary plugins that use published interfaces and sell a bundle. The engine and its modifications stay GPL; the features do not.
3. **Hosted services.** Cutweave is GPL, not AGPL. Running a modified engine as a service (§22 mentions cloud render) is not conveying (GPLv3 §0: "Mere interaction with a user through a computer network, with no transfer of a copy, is not conveying"), so the modifications need not be published.
4. **The plugin index.** If the project's index mirrors proprietary plugin binaries, the project itself distributes proprietary software and needs submission terms (license to redistribute, takedown, labelling, export control). WP-6A.09 mentions labelling only.

## Nits

### N1. `LICENSE` has one extra trailing newline

`LICENSE` is 35,150 bytes and ends with a blank line; the canonical text at <https://www.gnu.org/licenses/gpl-3.0.txt> appears to be 35,149 bytes without it. This needs to be checked against a fresh download: WP-0B.01's acceptance criterion is a hash match, and license detectors compare text. Not applied (no download was made in this review).

### N2. Legal-review routing misses files

GOVERNANCE.md says the legal reviewer covers "`LICENSE*`, `wit/`, `packages/plugin-sdk`"; CODEOWNERS matches. Add `packages/compose-runtime/`, `REUSE.toml`, `CONTRIBUTING.md` (licensing section), `schema/` and, once it exists, `docs/legal/`. Not applied (CODEOWNERS and GOVERNANCE changes need maintainer approval).

### N3. Do not add an Apache `NOTICE` file

WP-0B.02 asked for "`NOTICE` for the Apache part"; none exists, which is fine. Under Apache-2.0 §4(d), an existing `NOTICE` file must be reproduced by every downstream redistributor, including every plugin that bundles the SDK. Leaving it out keeps plugins lighter. Update the WBS note rather than add the file.

### N4. Non-operative wording in the operative text

The draft's §1 "Purpose" and its "in the same spirit as …" sentence sat among the terms. Applied: moved into a preamble marked "explanatory, not part of the terms", and the terms are numbered §0-§7 like GCC RLE 3.1.

### N5. Terminology drift

"GPL-3.0" versus "GPL-3.0-or-later" (README, product brief D4); "iframe RPC" (ADR-0010, ADR-0011) versus `WebContentsView` (architecture §15); `@cutweave/sdk` versus `@cutweave/plugin-sdk`; "ADR-011" versus "ADR-0011". README and the exception are fixed here; the ADRs and architecture are for the integrator.

### N6. "Intended to be" (CONTRIBUTING)

Covered by S10. Applied.

### N7. DCO identity

CONTRIBUTING does not say what name a sign-off must use. Many projects now accept a known identity rather than a legal name (the Linux kernel changed its wording in 2023). One sentence avoids arguments with pseudonymous contributors. Not applied; maintainers' call.

## Questions for counsel

1. Is the restructured grant (exception §1: combination, plugin terms, distribution together, Corresponding Source) an effective GPLv3 §7 additional permission in the jurisdictions where the maintainers and main distributors are based? Should it add Qt GPL Exception 1.0's source-offer condition?
2. Who holds copyright in the existing code (an individual or an organisation, given the organisational commit e-mail), and what document should record that holder's grant of the exception?
3. Is DCO sign-off, plus the CONTRIBUTING statement, plus a PR-template checkbox, plus per-file `WITH` notices, plus GitHub ToS §D.6, enough to bind external contributors to the exception, including to later versions that "only add permissions" and to designations under Published Interface item 9? Or is a short contributor agreement needed?
4. In-process plugins (WASM, QuickJS, WGSL) run in the same process as a GPL FFmpeg build (x264, x265, vid.stab). What is the realistic exposure for a proprietary plugin vendor, and which of B3's options (document, separate process, LGPL build) do you recommend?
5. May the project place generated schemas and types derived from GPL sources (`schema/`, `packages/protocol/src/generated/`) under MIT OR Apache-2.0 or treat them as Interface Material, given the inbound terms? Are such schemas protectable at all?
6. Is exception §3 (output) the right tool, or should built-in content simply be relicensed (CC0-1.0 / CC-BY-4.0)? Is the exclusion of source code from §3 drawn in the right place?
7. Is the condition in exception §2.2 (no restriction on recipients' GPL rights in Cutweave) enforceable and useful? Should the exception also restrict "required" proprietary plugins in open-core bundles, or closed replacement UIs (S14)?
8. Should the exception include a patent-peace condition? What should vendors bundling patent-licensed codec plugins with Cutweave do about GPLv3 §11 ¶¶ 5-7 and §12? Does SVT-AV1's AOMedia Patent License 1.0 raise any GPL issue? (Overlaps 7F.03.)
9. Trademark: clearance and registration of "Cutweave"; is a fork-renaming policy compatible with GPLv3 §7(e) and §10 as proposed in S9?
10. Which SPDX expression and notice should the files carry (`AdditionRef-…` versus a listed SPDX exception versus a per-file pointer), and is a top-level notice enough under GPLv3 §7's notice requirement?
11. With Cutweave GPL-3.0-or-later and the exception tied to §7 of version 3, is "applies under a later version to the extent it allows" sound? Should the project name a proxy under GPLv3 §14?
12. AI-authored code: how does limited copyrightability affect enforcement of the GPL and the exception, and is the DCO wording adequate for operator-certified agent output?
13. Plugin index: what terms are needed for hosting or mirroring third-party (including proprietary) plugins and for removing them?
14. If app-store distribution (Mac App Store, Microsoft Store) is ever considered, is it compatible with GPLv3 and the exception? (Not planned: §21 lists DMG, NSIS/MSI, AppImage, deb, rpm and Flathub.)

## Changes applied

| File | Change | Findings |
|---|---|---|
| [LICENSE-PLUGIN-EXCEPTION.md](../../LICENSE-PLUGIN-EXCEPTION.md) | Rewritten as "version 1.0", still **DRAFT, not in effect**: preamble marked non-operative; definitions of GPL (with later versions), Cutweave Project, Cutweave Code, Published Interface (nine items, correct package names, designation mechanism), Interface Material, Internals, Plugin; grant over combination, plugin terms, distribution together and Corresponding Source; conditions; output; limits (third-party GPL, trademark, patents); no-inference; removal per §7; adoption steps; reviewer notes | B2, B3, S1-S9, N4 |
| [CONTRIBUTING.md](../../CONTRIBUTING.md) | DCO sentence ties the sign-off to the license in each file; licensing section: permissive directories stated as fact, Apache-style inbound sentence, one-way code flow, trademark line, and the plugin-exception inbound paragraph rewritten (no opt-out, bounded consent, adoption before the first external merge) | B1, S9, S10, N6 |
| [README.md](../../README.md) | License paragraph no longer says the exception is in effect; says GPL-3.0-or-later | S11, N5 |

Not changed, for the integrator or maintainers: ADR-0011 amendment (B1, S4, S7, S11), architecture §3, §12, §15 and §21 (B3, S3, N5), `deny.toml` register (B3), PR template checkbox (B1), CODEOWNERS and GOVERNANCE (N2), CI check for permissive packages (S10), `TRADEMARKS.md` (S9), About screen and source offer (S13).

## Appendix: precedents compared

| Precedent | What it does | Lesson for Cutweave |
|---|---|---|
| GCC Runtime Library Exception 3.1 (`GCC-exception-3.1`) | §7 additional permission: Target Code combining the runtime library with Independent Modules may be propagated under any terms, if every compilation step was "Eligible". §2 "No Weakening of GCC Copyleft" | Structure (definitions, grant, no-weakening); a condition can steer behaviour; GCC used it to *discourage* proprietary compiler plugins, Cutweave the opposite |
| GNU Classpath exception (`Classpath-exception-2.0`) | Permission to link with "independent modules" (not derived from the library) and distribute the executable under terms of your choice, meeting each module's own license; downstream may extend or delete it | The grant must be about the combination and its distribution (B2); negative definition of the plugin (S6); simple removal (S1) |
| Qt GPL Exception 1.0 (`Qt-GPL-exception-1.0`, used with GPL-3.0) | Exception 1: output of the application in larger works; Exception 2: combine the application with plugins under terms of your choice, with a prominent source offer for the application | Closest analogue for a GPL application with proprietary plugins; includes an output permission (S7) |
| Autoconf exception, GPL font exception, Bison exception | Output that embeds parts of the program (configure scripts, fonts in documents, parser skeletons) is not covered | Rendered media with built-in assets needs a permission or permissive asset licenses (S7) |
| FSF template "linking over a controlled interface" (FAQ LinkingOverControlledInterface) | Permits independent modules that use only a named interface; framed as a §7 additional permission; only copyright holders can grant it, not for others' GPL code | B1 and B3 |
| `Linux-syscall-note` | Clarifies that user programs using normal system calls are not derived works | Clarify rather than grant for arm's-length clients (S5) |
| Blender (GPL) | No exception; the Blender Foundation's long-standing position is that Python add-ons using its API must be GPL-compatible, though they may be sold. Verify current wording | Without an exception, vendors face ambiguity (ADR-0011 option D) |
| GIMP (GPL-3.0-or-later) | Plug-ins are separate processes using `libgimp`, which is LGPL-3.0-or-later; proprietary plug-ins are possible. Verify | Permissive interface library plus process separation works (Cutweave L3, L4) |
| Audacity (GPLv3; many files GPL-2.0-or-later) | Hosts third-party VST, VST3, AU and LV2 plugins through standard interfaces; Muse Group added a CLA in 2021. Verify | Standard-interface plugins need no host-specific permission, but naming them avoids doubt (S3); CLA versus DCO trade-off (B1) |
| OBS Studio, Krita (GPL) | In-process native plugins linking the host library, no plugin exception; plugins are generally treated as needing GPL-compatible licenses. Verify | Exactly what ADR-0010 avoids by keeping native code out of process |

## References

- GNU GPL v3 text: [LICENSE](../../LICENSE) (§0, §1, §2, §5, §7, §9-§12, §14 cited above).
- FSF, *Frequently Asked Questions about the GNU Licenses*: [GPLPlugins](https://www.gnu.org/licenses/gpl-faq.html#GPLPlugins), [GPLPluginsInNF](https://www.gnu.org/licenses/gpl-faq.html#GPLPluginsInNF), [LinkingOverControlledInterface](https://www.gnu.org/licenses/gpl-faq.html#LinkingOverControlledInterface), [GPLIncompatibleLibs](https://www.gnu.org/licenses/gpl-faq.html#GPLIncompatibleLibs), [MereAggregation](https://www.gnu.org/licenses/gpl-faq.html#MereAggregation), [GPLOutput](https://www.gnu.org/licenses/gpl-faq.html#GPLOutput), [WhatCaseIsOutputGPL](https://www.gnu.org/licenses/gpl-faq.html#WhatCaseIsOutputGPL), [FontException](https://www.gnu.org/licenses/gpl-faq.html#FontException).
- FSF, [GCC Runtime Library Exception 3.1](https://www.gnu.org/licenses/gcc-exception-3.1.html) and its FAQ; FSF, GPLv3 rationale documents on §7.
- SPDX License List: [Classpath-exception-2.0](https://spdx.org/licenses/Classpath-exception-2.0.html), [Qt-GPL-exception-1.0](https://spdx.org/licenses/Qt-GPL-exception-1.0.html), [GCC-exception-3.1](https://spdx.org/licenses/GCC-exception-3.1.html), [Autoconf-exception-3.0](https://spdx.org/licenses/Autoconf-exception-3.0.html), [Linux-syscall-note](https://spdx.org/licenses/Linux-syscall-note.html).
- SPDX 3.0 license expressions, `AdditionRef-` syntax: <https://spdx.github.io/spdx-spec/v3.0.1/annexes/spdx-license-expressions/>.
- Developer Certificate of Origin 1.1: <https://developercertificate.org/>.
- GitHub Terms of Service §D.6, "Contributions Under Repository License": <https://docs.github.com/en/site-policy/github-terms/github-terms-of-service>.
- Apache License 2.0 (§3 patents, §4(d) NOTICE, §5 contributions, §6 trademarks): <https://www.apache.org/licenses/LICENSE-2.0>.
- Software Freedom Law Center, *Maintaining Permissive-Licensed Files in a GPL-Licensed Project* (2007), and *A Practical Guide to GPL Compliance* (2008).
- copyleft.org, *Copyleft and the GNU General Public License: A Comprehensive Tutorial and Guide* (chapters on GPLv3 §7 and §11).
- US Copyright Office, *Copyright and Artificial Intelligence, Part 2: Copyrightability* (29 January 2025): <https://www.copyright.gov/ai/>.
- *Google LLC v. Oracle America, Inc.*, 593 U.S. 1 (2021).
