// SPDX-License-Identifier: GPL-3.0-or-later
// The export queue row's menu and commands, registered once with the app's menu services.
import { menuServices } from "../menus/defaults.ts";
import { registerQueueMenu } from "./queueMenu.ts";

registerQueueMenu(menuServices);
