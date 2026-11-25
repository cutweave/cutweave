// SPDX-License-Identifier: GPL-3.0-or-later
// The app's menu services: one set of registries, with the framework's own entries in it.
import { registerBuiltinMenus } from "./builtins.ts";
import { createMenuServices } from "./services.ts";

export const menuServices = createMenuServices();
registerBuiltinMenus(menuServices);
