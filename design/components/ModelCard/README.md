ModelCard presents one generative model in AI Studio with its provider, capabilities, a cost estimate and a primary Generate button.

**When to use**
- The model picker in AI Studio (image, video, audio and voice models).
- Showing a model replaced or retired by its provider.
- Not for generated results (use AssetTile with the AI badge).

**Anatomy**
`.cw-model` (`surface-2`, `border-default`, `radius-lg`, `space-4` padding) > `__head`: `__name` (type: heading) + `__provider` ("via fal", type: caption, `text-muted`) > `__caps` chips (`.cw-chip.cw-chip--cap`: Image->Video, 5-10 s, 1080p) > `__foot`: CostChip + Generate (Button `--primary --lg`, `control-lg`).

**The consumer provides**
Model id and display name, aggregator provider, capability list, the cost estimate for the current parameters, deprecation info (`replacedBy`), and availability.

**States**
Default; hover/focus on the card is not interactive, only its buttons; deprecated (`data-state="deprecated"`: dashed border, name in `text-secondary`, a `warning` note with icon and the words "Deprecated, replaced by Kling 3.0", Generate disabled, a "Switch" button offered); loading estimate (cost chip shows a pending label); over budget (CostChip warning).

**Keyboard & accessibility**
Card is an `<article aria-label>`; buttons are in natural order (Switch, Generate). The deprecation notice uses icon + words and is read before the buttons. Capabilities are real text chips.

**Command mapping**
Generate dispatches `ai.generate {model, params}` only after an estimate is shown: the card first calls `ai.estimate {model, params}` and displays the CostChip. If the estimate exceeds the budget, generation requires approval (CostChip "Needs approval"). Agents use the same two commands.

**Do**
- Show the cost chip before the user can click Generate.
- Keep Generate the only marigold element on the card.
- Name the replacement model explicitly in the deprecated state.

**Don't**
- Don't use agent blue for the Generate button; AI provenance comes after generation.
- Don't hide deprecated models the current project already uses.
- Don't show more than three capability chips; expand in a details view.
