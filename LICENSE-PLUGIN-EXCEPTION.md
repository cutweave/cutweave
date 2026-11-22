# Cutweave Plugin Exception

> **DRAFT. Pending legal review. NOT YET IN EFFECT.**
>
> This text is a proposal drafted for work package WP-0B.01 (a human legal gate, ADR-011). It has not been reviewed by counsel or approved by the maintainers. Until it is approved and merged as final, Cutweave is licensed under the GNU General Public License version 3 alone, as in the `LICENSE` file, and nothing in this file grants any additional permission. Do not rely on it.

## 1. Purpose

Cutweave's core is licensed under GPL-3.0-or-later. We want people to be able to write plugins, effects, providers, templates and data packs for Cutweave under any license they choose, including proprietary ones, as long as those plugins only talk to Cutweave through its public, documented interfaces. This exception says so.

It is written as an "additional permission" under section 7 of the GNU GPL version 3, in the same spirit as the GCC Runtime Library Exception and the Classpath Exception.

## 2. Definitions

- **Cutweave** means the program and source files in this repository that are licensed under the GPL, other than the Published Interfaces listed below when they carry their own (MIT OR Apache-2.0) license.
- **Published Interface** means any of the following, in the form published in a Cutweave release:
  1. the WIT packages in the `wit/` directory;
  2. the `@cutweave/sdk` package;
  3. the `@cutweave/compose` package and its composition runtime API;
  4. the plugin manifest format (`cutweave-plugin.toml`);
  5. the command protocol: the documented commands, schemas and events exposed through the MCP server, the CLI and the JSON-RPC local API;
  6. declarative data formats for packs and effects that Cutweave documents as loadable (for example LUTs, fonts, caption styles, templates, macros and `effect.toml` with WGSL sources).
- **Plugin** means a work that extends, drives or provides content to Cutweave and that interacts with Cutweave **only** through Published Interfaces. This includes a WASM component built against the WIT packages, a UI or composition extension built with the SDK, an external program or agent that controls Cutweave through MCP, the CLI or JSON-RPC, and a data pack.
- **Internals** means everything else: Cutweave's Rust crates (`crates/`), its internal Rust, TypeScript or native APIs and ABIs, the engine's in-process memory, files or databases not documented as an interface, and the source code of the Cutweave application itself.

## 3. Additional permission

Under GPL-3.0 section 7, you are granted the following additional permission.

You may create, convey and license a Plugin under terms of your choice, including terms that are not compatible with the GPL, and including proprietary terms, provided that the Plugin interacts with Cutweave only through Published Interfaces.

For the avoidance of doubt, this includes the case where a Plugin is loaded, executed, sandboxed or invoked by Cutweave at run time, and the case where a Plugin includes or links the code of a Published Interface that is itself licensed under MIT OR Apache-2.0 (for example the SDK).

## 4. What this does not cover

1. A work that copies, links against, modifies or otherwise depends on **Internals** is not a Plugin and gets no permission from this exception. It remains subject to the GPL.
2. This exception covers the Plugin itself. It does not change the license of Cutweave, and any modified version of Cutweave you convey remains under the GPL.
3. It does not grant any right to Cutweave's names or logos, and does not affect the licenses of third-party components bundled with Cutweave (including FFmpeg) or of models and assets a Plugin downloads or ships.
4. Nothing here weakens the requirement that you comply with the GPL for the parts of Cutweave you convey.

## 5. Forks and downstream recipients

As permitted by GPL-3.0 section 7, a person who conveys a modified version of Cutweave may, at their option, remove this additional permission from their version, or from any part of it. The permission is retained for a modified version unless it is explicitly removed. If you remove it, you must say so in a prominent notice.

## 6. No warranty

The same disclaimer of warranty and limitation of liability as in the GPL applies.

## 7. Notes for reviewers (to remove before adoption)

- Compare the wording against the GCC Runtime Library Exception 3.1 and the Classpath Exception, and decide whether a "Compilation Process"-style definition is useful here.
- Confirm that out-of-process control (MCP, CLI, JSON-RPC) being a Plugin matches the intent of ADR-011, and whether a stable "version" of the Published Interfaces should be pinned.
- Confirm the final mechanism: a SPDX-style `GPL-3.0-or-later WITH Cutweave-plugin-exception` identifier, license headers and the notice text that must be added to the `LICENSE` file or per-file headers once approved. `LICENSE` must stay the verbatim GPL-3.0 text.
- Record the outcome: "approved by counsel" or "maintainer-approved, counsel review pending before 1.0" (architecture §21).
