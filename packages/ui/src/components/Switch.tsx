// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixSwitch from "@radix-ui/react-switch";
import { cx } from "./cx.ts";

export interface SwitchProps {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean | undefined;
  id?: string | undefined;
  /** Names the switch when there is no visible `<label htmlFor>` / `aria-labelledby`. */
  "aria-label"?: string | undefined;
  "aria-labelledby"?: string | undefined;
  className?: string | undefined;
}

/** On/off setting with immediate effect; on is marigold. Space toggles. */
export function Switch({ checked, onCheckedChange, className, ...rest }: SwitchProps) {
  return (
    <RadixSwitch.Root
      checked={checked}
      onCheckedChange={onCheckedChange}
      className={cx("cw-switch", className)}
      {...rest}
    >
      <RadixSwitch.Thumb className="cw-switch__thumb" />
    </RadixSwitch.Root>
  );
}
