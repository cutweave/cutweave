// SPDX-License-Identifier: GPL-3.0-or-later
import { Icon } from "../icons/index.ts";
import { cx } from "./cx.ts";

interface CostBase {
  /** ISO 4217 code for `amount`. Defaults to USD. */
  currency?: string | undefined;
  className?: string | undefined;
}

export type CostChipProps = CostBase &
  (
    | { state: "estimate"; amount: number }
    | { state: "over-budget"; amount: number }
    | { state: "approval"; amount?: number | undefined }
    | { state: "free" }
    | { state: "estimating" }
  );

/** "~$0.40": estimates are always prefixed with a tilde. */
export function formatCost(amount: number, currency = "USD"): string {
  return `~${new Intl.NumberFormat("en-US", { style: "currency", currency }).format(amount)}`;
}

/** What an AI action will cost before it runs. Static; every state carries a number or a word. */
export function CostChip(props: CostChipProps) {
  const { className, currency } = props;
  let data: string | undefined;
  let icon: "check" | "triangle-alert" | "sparkles" | undefined;
  let amount: string | undefined;
  let words: string | undefined;
  switch (props.state) {
    case "estimate":
      amount = formatCost(props.amount, currency);
      break;
    case "free":
      icon = "check";
      words = "Free (local)";
      break;
    case "over-budget":
      data = "warning";
      icon = "triangle-alert";
      amount = formatCost(props.amount, currency);
      words = "Over budget";
      break;
    case "approval":
      data = "approval";
      icon = "sparkles";
      if (props.amount !== undefined) amount = formatCost(props.amount, currency);
      words = "Needs approval";
      break;
    case "estimating":
      words = "Estimating";
      break;
  }
  return (
    <span className={cx("cw-cost label", className)} data-state={data}>
      {icon ? <Icon name={icon} size="sm" /> : null}
      {words}
      {amount ? <span className="cw-mono">{amount}</span> : null}
    </span>
  );
}
