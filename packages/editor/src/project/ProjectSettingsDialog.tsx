// SPDX-License-Identifier: GPL-3.0-or-later
// File > Project settings… (WP-2H.08): the settings saved in the project, apart from the app's
// Settings. A schema-driven form over the engine's `ProjectSettings`: every change is one
// `project.set_settings` (one undo step, "Change project settings"), and since the form shows the
// document, a change an agent makes (or an undo) shows at once. "Use as defaults for new
// projects" copies the values that have an app preference into Settings.
import { Button, Dialog, DialogNote, type OnChange, SchemaForm } from "@cutweave/ui";
import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { engineStore } from "../engine/engineStore.ts";
import { preferences } from "../prefs/prefsStore.ts";
import { useDocStore } from "../state/docStore.ts";
import { translator } from "../timeline/errors.ts";
import {
  applySetting,
  defaultsOf,
  projectSettingsStore,
  settingsForm,
  settingsSchema,
} from "./projectSettings.ts";

const deps = { engine: engineStore };

export function ProjectSettingsDialog() {
  const { t } = useTranslation();
  const state = useStore(projectSettingsStore);
  const settings = useDocStore((s) => s.project?.settings);
  // A number half typed or a colour being picked shows until it is committed.
  const [draft, setDraft] = useState<Readonly<Record<string, unknown>>>({});
  const schema = useMemo(() => settingsSchema(), []);
  const form = useMemo(
    () => (settings === undefined ? null : settingsForm(schema, settings, translator(t))),
    [schema, settings, t],
  );
  const value = useMemo(() => {
    if (form === null) return {};
    let shown: Record<string, unknown> = form.value;
    for (const [path, next] of Object.entries(draft)) {
      const [head, ...rest] = path.split(".");
      if (head === undefined) continue;
      if (rest.length === 0) shown = { ...shown, [head]: next };
      else if (rest.length === 1)
        shown = {
          ...shown,
          [head]: { ...(shown[head] as Record<string, unknown>), [rest[0] as string]: next },
        };
    }
    return shown;
  }, [form, draft]);

  const onChange = useCallback<OnChange>(
    (path, next, meta) => {
      if (meta.gesture === "start" || meta.gesture === "update") {
        setDraft((d) => ({ ...d, [path]: next }));
        return;
      }
      const settle = (): void => setDraft(({ [path]: _gone, ...rest }) => rest);
      void applySetting(deps, path, next, t("projectSettings.refused")).finally(settle);
    },
    [t],
  );

  if (!state.open) return null;
  const close = () => projectSettingsStore.getState().close();
  const useAsDefaults = () => {
    if (settings === undefined) return;
    const next = defaultsOf(settings);
    void Promise.all([
      preferences.set("editing.still_duration", next.stillSeconds),
      preferences.set("media.proxy_policy", next.proxyPolicy),
    ]).then(
      () =>
        projectSettingsStore.getState().set({
          notice: t("projectSettings.defaultsSaved", {
            seconds: String(next.stillSeconds),
            proxies: t(`settings.rows.proxyPolicy.${next.proxyPolicy}`),
          }),
        }),
      () => projectSettingsStore.getState().set({ notice: t("projectSettings.defaultsFailed") }),
    );
  };

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) close();
      }}
      title={t("projectSettings.title")}
      description={t("projectSettings.hint")}
      footer={
        <>
          <Button
            variant="secondary"
            size="lg"
            disabled={form === null}
            onClick={useAsDefaults}
            data-testid="project-settings-defaults"
          >
            {t("projectSettings.useAsDefaults")}
          </Button>
          <Button variant="primary" size="lg" onClick={close}>
            {t("projectSettings.close")}
          </Button>
        </>
      }
    >
      {form === null ? (
        <DialogNote>{t("projectSettings.noProject")}</DialogNote>
      ) : (
        <div data-testid="project-settings">
          <SchemaForm
            aria-label={t("projectSettings.formLabel")}
            schema={form.schema}
            hints={form.hints}
            value={value}
            errors={state.errors}
            onChange={onChange}
          />
        </div>
      )}
      {state.notice === null ? null : (
        <DialogNote>
          <span role="status">{state.notice}</span>
        </DialogNote>
      )}
    </Dialog>
  );
}
