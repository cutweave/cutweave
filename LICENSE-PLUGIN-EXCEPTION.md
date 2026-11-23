# Cutweave Plugin Exception, version 1.0

> **DRAFT. Pending legal review. NOT YET IN EFFECT.**
>
> This text is a proposal drafted for work package WP-0B.01 (a human legal gate, ADR-0011) and revised after a first-pass review ([docs/legal/first-pass-review-2026-11-23.md](docs/legal/first-pass-review-2026-11-23.md)). It has not been reviewed by counsel or approved by the maintainers. Until it is approved and merged as final, Cutweave is licensed under the GNU General Public License version 3 or later alone, as in the `LICENSE` file, and nothing in this file grants any additional permission. Do not rely on it.

## Preamble (explanatory, not part of the terms)

Cutweave's core is licensed under GPL-3.0-or-later. The Cutweave project wants people to be able to write plugins, effects, providers, templates, data packs and agents for Cutweave under any license they choose, including proprietary ones, and to distribute them alongside Cutweave, as long as those works use only Cutweave's published interfaces. The editor and engine themselves stay copyleft.

This exception is an additional permission under section 7 of the GPL. Its structure follows the GCC Runtime Library Exception 3.1, the GNU Classpath exception and the Qt GPL Exception 1.0. Sections 0 to 7 below are the terms.

## 0. Definitions

- **GPL** means the GNU General Public License, version 3, or any later version published by the Free Software Foundation that you choose to apply under its section 14.
- **Cutweave Project** means the maintainers of Cutweave as described in `GOVERNANCE.md`.
- **Cutweave** means the Cutweave program as released by the Cutweave Project, and any modified version of it that retains this exception.
- **Cutweave Code** means material in Cutweave whose copyright holders license it under the GPL together with this exception, as stated in the material's license notice or in a notice that covers it. Cutweave Code does not include material under other licenses, such as the MIT OR Apache-2.0 parts of the Published Interfaces, or third-party components distributed with Cutweave, such as FFmpeg and the libraries linked into it.
- **Published Interface** means any of the following, as released by the Cutweave Project in a version of Cutweave, together with its documentation, schemas and examples and any code generated from it:
  1. the WIT packages in `wit/`;
  2. the plugin SDK in `packages/plugin-sdk` and the composition runtime in `packages/compose-runtime`, under whatever package names they are published (currently `@cutweave/plugin-sdk` and `@cutweave/compose-runtime`);
  3. the plugin manifest format (`cutweave-plugin.toml`);
  4. the declarative formats that Cutweave documents as loadable: data packs (for example LUTs, fonts, stickers, audio, caption styles, text presets, templates, macros and export presets), GPU effects (`effect.toml` with WGSL sources, including the binding code Cutweave generates for them) and compositions;
  5. the UI extension interface: the contribution points and the message protocol between a UI extension and Cutweave;
  6. the command protocol: the documented commands, queries, events and their schemas, as exposed through the JSON-RPC local API, the MCP server and the CLI, including the generated files in `schema/` and `packages/protocol/src/generated/`;
  7. the documented project file format;
  8. the interface Cutweave uses to host out-of-process native plugins, and the third-party standard plugin interfaces Cutweave hosts through it (for example CLAP, VST3 or OpenFX); and
  9. any other interface that the Cutweave Project designates as a Published Interface in the documentation of a release.

  An interface added or changed by anyone other than the Cutweave Project is not a Published Interface.
- **Interface Material** means the parts of a Published Interface that are Cutweave Code, such as schemas, generated type definitions and bindings, generated binding code and documented examples.
- **Internals** means everything in Cutweave that is not a Published Interface, including its Rust crates and their APIs and ABIs, its internal TypeScript, native and inter-process interfaces, the engine's in-process memory, files and databases that are not documented as an interface, and the source code of the Cutweave application.
- **Plugin** means a work that extends, drives, controls or supplies content to Cutweave and that (a) interacts with Cutweave only through Published Interfaces, and (b) contains no Cutweave Code other than Interface Material and is not otherwise based on Cutweave Code. Examples are a WASM component built against the WIT packages, a UI extension or composition built with the SDK, a GPU effect, a data pack, an out-of-process native plugin, and an external program or agent that controls Cutweave through MCP, the CLI or JSON-RPC.

## 1. Grant of additional permissions

The copyright holders of Cutweave Code grant you the following additional permissions under section 7 of the GPL. If you apply a later version of the GPL, they apply as additional permissions under that version to the extent it allows them.

1. **Combination.** You may combine Cutweave Code with Plugins, whether Cutweave loads, links, executes, sandboxes or invokes a Plugin at run time or the Plugin controls Cutweave from another process, and you may convey the result. The GPL does not require the Plugins, or the combination as a whole, to be licensed under the GPL; the Cutweave Code in the combination remains under the GPL.
2. **Plugin terms.** You may license and convey a Plugin, including any Interface Material it contains, under terms of your choice, including proprietary terms.
3. **Distribution together.** You may convey a Plugin together with Cutweave, for example in the same installer, archive, package, plugin index or device, provided that you convey Cutweave itself under the GPL, with or without this exception.
4. **Corresponding Source.** The Corresponding Source of Cutweave, as the GPL defines it, does not include the source code of a Plugin.

## 2. Conditions

The permissions in section 1 apply to a Plugin only while:

1. the Plugin meets the definition of Plugin in section 0; and
2. the terms under which you convey the Plugin, or Cutweave together with it, do not restrict any recipient's exercise of the rights the GPL grants for Cutweave.

You must still meet every condition of the GPL for the Cutweave Code you convey.

## 3. Output

Rendered video, audio, still images, captions and similar media produced by running Cutweave are not made subject to the GPL merely because they contain copies of Cutweave Code that Cutweave places in its output, such as built-in templates, effects, presets, fonts or other assets. You may use and convey such output under terms of your choice. This section does not apply to source code (for example a composition, effect or template copied or adapted from Cutweave Code), and it does not affect any other reason why output might be subject to the GPL or another license.

## 4. What this does not cover

1. A work that copies, links against, modifies or otherwise depends on Internals, or that contains Cutweave Code other than Interface Material, is not a Plugin and gets no permission from this exception. It remains subject to the GPL.
2. This exception is granted only by the copyright holders of Cutweave Code. It grants no permission for third-party components distributed with or used by Cutweave, including FFmpeg and the libraries linked into it, some of which are licensed under the GPL. Those components remain under their own licenses, and section 7 of the GPL provides that where additional permissions apply to only part of a program, the program as a whole remains governed by the GPL without regard to them. This exception does not decide whether combining a Plugin with a Cutweave build that includes such components needs permission from their copyright holders.
3. This exception does not change the license of Cutweave. Any modified version of Cutweave you convey remains under the GPL.
4. It grants no right to use the Cutweave name, logos or other trademarks.
5. It grants no patent license. The patent licenses for Cutweave are those in section 11 of the GPL and, for parts licensed under MIT OR Apache-2.0 when you choose Apache-2.0, those in section 3 of Apache-2.0.
6. It does not affect the licenses of models, assets or services that a Plugin uses, downloads or ships.

## 5. No inference and no weakening

This exception does not imply that a work that communicates with Cutweave at arm's length, for example over MCP, the CLI or a network protocol, would otherwise be a work based on Cutweave, nor does it imply that any work is not. Nothing in this exception weakens the GPL's requirements for Cutweave itself.

## 6. Removal and modified versions

When you convey a copy of Cutweave, modified or not, you may remove this exception from that copy or from any part of it, as section 7 of the GPL permits. If you do not remove it, it continues to apply. You may also extend it to material you add to Cutweave, if you have or can give the copyright permission to do so. If you remove it, please say so prominently, so that plugin authors are not misled.

## 7. No warranty

This exception gives no warranty. Sections 15, 16 and 17 of the GPL apply.

## Applying the exception (instructions, not part of the terms)

When the maintainers adopt this text after legal review:

1. Remove the DRAFT banner and the reviewer notes, and record the outcome ("approved by counsel" or "maintainer-approved, counsel review pending before 1.0", architecture §21).
2. Change the license notice of every file that is Cutweave Code, so that each file states the additional terms or says where to find them, as section 7 of the GPL requires, and so that a DCO sign-off ("the open source license indicated in the file") covers them. Proposed: `SPDX-License-Identifier: GPL-3.0-or-later WITH AdditionRef-Cutweave-plugin-exception-1.0`, the same expression in `REUSE.toml`, and `cargo xtask license-headers` updated to enforce it. The identifier is to be confirmed, and tool support (REUSE, cargo-deny, npm) checked; the fallback is to keep `GPL-3.0-or-later` in package metadata and add a one-line notice pointing to this file.
3. Keep `LICENSE` as the verbatim GPL-3.0 text.
4. Update `README.md`, `CONTRIBUTING.md`, ADR-0011, the Cargo and npm `license` fields, and the application's About box.

## Notes for reviewers (to remove before adoption)

- The findings behind this revision, the proposed text for each and the questions for counsel are in [docs/legal/first-pass-review-2026-11-23.md](docs/legal/first-pass-review-2026-11-23.md).
- Decide the position on bundled third-party GPL code (FFmpeg with x264, x265 and vid.stab) and in-process plugins before adoption (review finding B3).
- Confirm that out-of-process control (MCP, CLI, JSON-RPC) being a Plugin, and the explicit permission to ship a Plugin together with Cutweave, match the intent of ADR-0011 (review findings S5 and S14).
- Confirm the designation mechanism in Published Interface item 9, and whether contributors' consent covers it (review findings B1 and S3).
- Confirm section 3 (output), or replace it by licensing built-in content separately (review finding S7).
