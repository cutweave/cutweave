// SPDX-License-Identifier: GPL-3.0-or-later
// The app's menu services: one set of registries, with the framework's own entries in it and the
// surface tables of today's panels (WP-2H.13).
import { registerBuiltinMenus } from "./builtins.ts";
import { createMenuServices } from "./services.ts";
import { registerSurfaceMenus } from "./surfaces/index.ts";

export const menuServices = createMenuServices();
registerBuiltinMenus(menuServices);
registerSurfaceMenus(menuServices);
