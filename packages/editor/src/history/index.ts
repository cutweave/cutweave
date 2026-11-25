// SPDX-License-Identifier: GPL-3.0-or-later
// The History panel's menus and commands, registered once with the app's menu services.
import { menuServices } from "../menus/defaults.ts";
import { registerHistoryCommands } from "./commands.ts";

registerHistoryCommands(menuServices);
