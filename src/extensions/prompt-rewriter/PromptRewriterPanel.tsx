import { useEffect, useId, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  SettingsBlock,
  SettingsCard,
  SettingsRow,
  SettingsSwitchRow,
} from "@/components/settings/SettingsLayout";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useUnsavedChangesTracker } from "@/lib/unsavedChanges";
import { extractErrorMessage } from "@/utils/errorUtils";
import { promptRewriterApi, promptRewriterKeys } from "./api";
import type { Config, JsonValue, Route, RouteConfig, Status } from "./types";

const ROUTES: Route[] = ["claudeCode", "claudeDesktop"];
type Editor = "system" | "userInstructions";
type Originals = Record<Route, Record<Editor, string | null>>;
type Confirmation =
  | { kind: "reload" }
  | { kind: "load" | "reset"; editor: Editor };

function originalsFrom(status: Status): Originals {
  return Object.fromEntries(
    ROUTES.map((route) => {
      const observation = status.observations[route];
      return [
        route,
        {
          // Saved replacements never stand in for a real request observation.
          system:
            observation.state === "captured" || observation.capturedAt !== null
              ? observation.system
              : null,
          userInstructions:
            status.sources[route].state === "ready"
              ? status.sources[route].content
              : null,
        },
      ];
    }),
  ) as Originals;
}

function FieldSelect<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  const id = useId();
  return (
    <div className="min-w-0 space-y-1.5">
      <Label htmlFor={id}>{label}</Label>
      <Select value={value} onValueChange={onChange}>
        <SelectTrigger id={id} aria-label={label}>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {options.map((option) => (
            <SelectItem key={option.value} value={option.value}>
              {option.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

export function PromptRewriterPanel() {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [savedAt, setSavedAt] = useState<number | null>(null);
  const status = useQuery({
    queryKey: promptRewriterKeys.status,
    queryFn: promptRewriterApi.get,
    retry: false,
    refetchOnWindowFocus: false,
    refetchInterval: (query) =>
      query.state.data?.available &&
      ROUTES.some(
        (route) => query.state.data?.observations[route].state === "waiting",
      )
        ? 1000
        : false,
  });
  // A get/cache refresh is not a save, even if the configuration is unchanged.
  const saved = savedAt !== null && savedAt === status.dataUpdatedAt;
  // Clean editors follow the complete shared Status query. Only a user edit
  // creates an independent draft; background observations cannot erase it.
  const [editedDraft, setDraft] = useState<Config | null>(null);
  const [draftOriginals, setDraftOriginals] = useState<Originals | null>(null);
  const draft = editedDraft ?? status.data?.config ?? null;
  const dirty = editedDraft !== null;
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [route, setRoute] = useState<Route>("claudeDesktop");
  const format = "anthropic";
  const [body, setBody] = useState("{}");
  const [jsonError, setJsonError] = useState<string | null>(null);
  const id = useId();
  const { onInputCapture: markDirty } = useUnsavedChangesTracker(dirty);
  useEffect(() => {
    if (dirty) markDirty();
  }, [dirty, markDirty]);
  useEffect(() => {
    if (!dirty) return;
    const beforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", beforeUnload);
    return () => window.removeEventListener("beforeunload", beforeUnload);
  }, [dirty]);

  const publish = (result: Status) =>
    client.setQueryData(promptRewriterKeys.status, result);
  const cancelGet = () =>
    client.cancelQueries({ queryKey: promptRewriterKeys.status });
  const preview = useMutation({ mutationFn: promptRewriterApi.preview });
  const save = useMutation({
    mutationFn: promptRewriterApi.save,
    onMutate: cancelGet,
    onSuccess: (result) => {
      publish(result);
      if (result.error || !result.available) return;
      setDraft(null);
      setDraftOriginals(null);
      setSavedAt(
        client.getQueryState(promptRewriterKeys.status)?.dataUpdatedAt ?? null,
      );
      preview.reset();
    },
  });
  const reload = useMutation({
    mutationFn: promptRewriterApi.reload,
    onMutate: async () => {
      await cancelGet();
      preview.reset();
      setJsonError(null);
      save.reset();
      refresh.reset();
      capture.reset();
      setSavedAt(null);
    },
    onSuccess: (result) => {
      // Domain errors still carry authoritative runtime state and path. A
      // same-path failure retains the last good config; a new path may disable it.
      publish(result);
      if (!result.error && result.available) {
        setDraft(null);
        setDraftOriginals(null);
      }
    },
  });
  const refresh = useMutation({
    mutationFn: promptRewriterApi.refreshSources,
    onMutate: cancelGet,
    onSuccess: (result) => {
      publish(result);
      setSavedAt(null);
      preview.reset();
    },
  });
  const capture = useMutation({
    mutationFn: promptRewriterApi.capture,
    onMutate: cancelGet,
    onSuccess: (result) => {
      publish(result);
      setSavedAt(null);
    },
  });
  const busy =
    save.isPending ||
    reload.isPending ||
    refresh.isPending ||
    capture.isPending ||
    preview.isPending;
  const errorText = (error: unknown) =>
    extractErrorMessage(error) || t("promptRewriter.unknownError");
  const change = (next: Config) => {
    const clean = JSON.stringify(next) === JSON.stringify(status.data?.config);
    if (!editedDraft && status.data)
      setDraftOriginals(originalsFrom(status.data));
    if (clean) setDraftOriginals(null);
    setDraft(clean ? null : next);
    setSavedAt(null);
    save.reset();
    reload.reset();
    preview.reset();
    setJsonError(null);
  };
  const changeRoute = (patch: Partial<RouteConfig>) => {
    if (!draft) return;
    change({
      ...draft,
      routes: {
        ...draft.routes,
        [route]: { ...draft.routes[route], ...patch },
      },
    });
  };
  const perform = (action: Confirmation) => {
    if (action.kind === "reload") {
      reload.mutate();
      return;
    }
    if (!draft || !status.data) return;
    const target = draft.routes[route][action.editor];
    if (action.kind === "reset") {
      changeRoute({
        [action.editor]: { ...target, enabled: false, replacement: null },
      });
    } else {
      const original = originalsFrom(status.data)[route][action.editor];
      if (original !== null)
        changeRoute({ [action.editor]: { ...target, replacement: original } });
    }
  };
  const requestAction = (action: Confirmation) =>
    dirty ? setConfirmation(action) : perform(action);
  const startPreview = () => {
    if (!draft) return;
    preview.reset();
    setJsonError(null);
    let parsed: JsonValue;
    try {
      parsed = JSON.parse(body) as JsonValue;
    } catch (error) {
      setJsonError(errorText(error));
      return;
    }
    preview.mutate({ config: draft, route, format, body: parsed });
  };
  const time = (timestamp: number | null) =>
    timestamp === null
      ? t("promptRewriter.notAvailable")
      : new Date(timestamp).toLocaleString();

  if (!status.isError && (status.isPending || status.data?.available === false))
    return null;

  const originals = status.data ? originalsFrom(status.data) : null;
  const source = status.data?.sources[route];
  const observation = status.data?.observations[route];
  const renderEditor = (editor: Editor) => {
    if (!draft || !status.data || !originals || !source || !observation)
      return null;
    const target = draft.routes[route][editor];
    const original = originals[route][editor];
    const editableOriginal =
      (dirty ? draftOriginals : originals)?.[route][editor] ?? null;
    return (
      <section
        aria-label={t(`promptRewriter.editors.${editor}`)}
        className="space-y-3 rounded-panel border border-border bg-surface p-4"
      >
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h3 className="text-body font-medium text-fg-1">
            {t(`promptRewriter.editors.${editor}`)}
          </h3>
          {editor === "system" ? (
            <Button
              variant="neutral"
              size="compact"
              disabled={observation.state === "waiting"}
              onClick={() => capture.mutate(route)}
            >
              {t("promptRewriter.capture")}
            </Button>
          ) : (
            <Button
              variant="neutral"
              size="compact"
              onClick={() => refresh.mutate()}
            >
              {t("promptRewriter.refreshSources")}
            </Button>
          )}
        </div>
        {editor === "system" ? (
          <div className="space-y-1 text-caption text-fg-2">
            <p>{t(`promptRewriter.captureStates.${observation.state}`)}</p>
            <p>{t("promptRewriter.captureHint")}</p>
            {observation.capturedAt !== null && (
              <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 break-all">
                <dt>{t("promptRewriter.capturedAt")}</dt>
                <dd>{time(observation.capturedAt)}</dd>
                <dt>{t("promptRewriter.provider")}</dt>
                <dd>
                  {observation.providerId ??
                    t("promptRewriter.providerNotSelected")}
                </dd>
                <dt>{t("promptRewriter.model")}</dt>
                <dd>{observation.model ?? t("promptRewriter.notAvailable")}</dd>
                <dt>{t("promptRewriter.format")}</dt>
                <dd>
                  {observation.format
                    ? t(`promptRewriter.formats.${observation.format}`)
                    : t("promptRewriter.notAvailable")}
                </dd>
                <dt>{t("promptRewriter.requestId")}</dt>
                <dd>
                  {observation.requestId ?? t("promptRewriter.notAvailable")}
                </dd>
                <dt>{t("promptRewriter.systemApplied")}</dt>
                <dd>
                  {observation.systemApplied
                    ? t("promptRewriter.applied")
                    : t("promptRewriter.notApplied")}
                </dd>
              </dl>
            )}
            {observation.error && (
              <p role="alert" className="text-danger-text">
                {observation.error}
              </p>
            )}
          </div>
        ) : (
          <div className="space-y-2 text-caption text-fg-2">
            <label className="block space-y-1.5">
              <span>{t("promptRewriter.sourcePath")}</span>
              <Input
                value={draft.routes[route].userInstructions.sourcePath ?? ""}
                placeholder={t("promptRewriter.defaultPath")}
                onChange={(event) =>
                  changeRoute({
                    userInstructions: {
                      ...draft.routes[route].userInstructions,
                      sourcePath: event.target.value || null,
                    },
                  })
                }
              />
            </label>
            <p>{t("promptRewriter.sourcePathHint")}</p>
            <p className="break-all">
              {t("promptRewriter.actualPath")}:{" "}
              {source.path ?? t("promptRewriter.notAvailable")}
            </p>
            <p>{t(`promptRewriter.sourceStates.${source.state}`)}</p>
            <p>
              {t("promptRewriter.loadedAt")}: {time(source.loadedAt)}
            </p>
            <p>
              {t("promptRewriter.fileIdentification")}:{" "}
              {t(`promptRewriter.fileStates.${observation.fileStatus}`)}
            </p>
            <p>
              {t("promptRewriter.userApplied")}:{" "}
              {observation.userApplied
                ? t("promptRewriter.applied")
                : t("promptRewriter.notApplied")}
            </p>
            {source.error && (
              <p role="alert" className="text-danger-text">
                {source.error}
              </p>
            )}
          </div>
        )}
        <label className="block space-y-1.5 text-body text-fg-1">
          <span>{t(`promptRewriter.originals.${editor}`)}</span>
          <Textarea
            readOnly
            className="min-h-[120px] font-mono text-xs"
            value={original ?? ""}
            placeholder={t(
              editor === "system"
                ? "promptRewriter.noSystemOriginal"
                : "promptRewriter.noFileOriginal",
            )}
          />
        </label>
        {original === "" && (
          <p className="text-caption text-fg-2">
            {t(
              editor === "system"
                ? "promptRewriter.emptyOriginal"
                : "promptRewriter.emptyFileOriginal",
            )}
          </p>
        )}
        <SettingsSwitchRow
          label={t(`promptRewriter.enableTargets.${editor}`)}
          checked={target.enabled}
          onCheckedChange={(enabled) =>
            changeRoute({ [editor]: { ...target, enabled } })
          }
        />
        <label className="block space-y-1.5 text-body text-fg-1">
          <span>{t(`promptRewriter.replacements.${editor}`)}</span>
          <Textarea
            className="min-h-[180px] font-mono text-xs"
            value={target.replacement ?? editableOriginal ?? ""}
            placeholder={t("promptRewriter.replacementPlaceholder")}
            onChange={(event) =>
              changeRoute({
                [editor]: { ...target, replacement: event.target.value },
              })
            }
          />
        </label>
        <p className="text-caption text-fg-2">
          {t(
            target.replacement === null
              ? "promptRewriter.inherited"
              : target.replacement === ""
                ? "promptRewriter.emptyReplacement"
                : "promptRewriter.fullReplacement",
          )}
        </p>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="neutral"
            size="compact"
            disabled={original === null}
            onClick={() => requestAction({ kind: "load", editor })}
          >
            {t("promptRewriter.loadOriginal")}
          </Button>
          <Button
            variant="quiet"
            size="compact"
            disabled={!target.enabled && target.replacement === null}
            onClick={() => requestAction({ kind: "reset", editor })}
          >
            {t("promptRewriter.resetTarget")}
          </Button>
        </div>
      </section>
    );
  };

  return (
    <SettingsBlock title={t("promptRewriter.title")}>
      {status.isError && (
        <div className="space-y-2">
          <p role="alert" className="text-caption text-danger-text">
            {t("promptRewriter.loadFailed", { error: errorText(status.error) })}
          </p>
          <Button
            variant="neutral"
            disabled={status.isFetching}
            onClick={() => void status.refetch()}
          >
            {t("promptRewriter.retry")}
          </Button>
        </div>
      )}
      {draft && status.data?.available && (
        <>
          <p className="text-caption text-fg-2">
            {t("promptRewriter.scopeHint")}
          </p>
          <fieldset disabled={busy} className="min-w-0 space-y-3">
            <SettingsCard>
              <SettingsSwitchRow
                label={t("promptRewriter.enabled")}
                checked={draft.enabled}
                onCheckedChange={(enabled) => change({ ...draft, enabled })}
              />
              <SettingsRow
                label={t("promptRewriter.configPath")}
                description={status.data.path}
              />
            </SettingsCard>
            {status.data.error && (
              <p role="alert" className="text-caption text-danger-text">
                {t("promptRewriter.configError", { error: status.data.error })}
              </p>
            )}
            <FieldSelect
              label={t("promptRewriter.route")}
              value={route}
              options={ROUTES.map((value) => ({
                value,
                label: t(`promptRewriter.routes.${value}`),
              }))}
              onChange={(value) => {
                setRoute(value);
                preview.reset();
                setJsonError(null);
              }}
            />
            <SettingsCard>
              <SettingsSwitchRow
                label={t("promptRewriter.routeEnabled")}
                checked={draft.routes[route].enabled}
                onCheckedChange={(enabled) => changeRoute({ enabled })}
              />
            </SettingsCard>
            {renderEditor("system")}
            {renderEditor("userInstructions")}
            <div className="flex flex-wrap items-center gap-3">
              <Button
                variant="solid"
                disabled={!dirty && !status.data.error}
                onClick={() => {
                  setSavedAt(null);
                  save.mutate(draft);
                }}
              >
                {t("promptRewriter.save")}
              </Button>
              <Button
                variant="neutral"
                onClick={() => requestAction({ kind: "reload" })}
              >
                {t("promptRewriter.reload")}
              </Button>
              <span role="status" className="text-caption text-fg-2">
                {t(
                  dirty
                    ? "promptRewriter.dirty"
                    : saved
                      ? "promptRewriter.saved"
                      : "promptRewriter.loaded",
                )}
              </span>
            </div>
          </fieldset>
          {(
            [
              [save, "saveFailed"],
              [reload, "reloadFailed"],
              [refresh, "refreshFailed"],
              [capture, "captureFailed"],
            ] as const
          ).map(
            ([mutation, key]) =>
              mutation.isError && (
                <p
                  key={key}
                  role="alert"
                  className="text-caption text-danger-text"
                >
                  {t(`promptRewriter.${key}`, {
                    error: errorText(mutation.error),
                  })}
                </p>
              ),
          )}
          <details className="space-y-3 rounded-panel border border-border bg-surface p-4">
            <summary className="cursor-pointer text-body font-medium text-fg-1">
              {t("promptRewriter.preview")}
            </summary>
            <fieldset disabled={busy} className="min-w-0 space-y-3">
              <p className="text-caption text-fg-2">
                {t("promptRewriter.previewHint")}
              </p>
              <p className="text-caption text-fg-2">
                {t("promptRewriter.route")}:{" "}
                {t(`promptRewriter.routes.${route}`)}
              </p>
              <p className="text-caption text-fg-2">
                {t("promptRewriter.format")}:{" "}
                {t(`promptRewriter.formats.${format}`)}
              </p>
              <Label htmlFor={`${id}-body`}>{t("promptRewriter.body")}</Label>
              <Textarea
                id={`${id}-body`}
                className="min-h-[160px] font-mono text-xs"
                value={body}
                aria-invalid={!!jsonError}
                onChange={(event) => {
                  setBody(event.target.value);
                  setJsonError(null);
                  preview.reset();
                }}
              />
              <Button variant="neutral" onClick={startPreview}>
                {t("promptRewriter.runPreview")}
              </Button>
            </fieldset>
            {jsonError && (
              <p role="alert" className="text-caption text-danger-text">
                {t("promptRewriter.invalidJson", { error: jsonError })}
              </p>
            )}
            {preview.isError && (
              <p role="alert" className="text-caption text-danger-text">
                {t("promptRewriter.previewFailed", {
                  error: errorText(preview.error),
                })}
              </p>
            )}
            {preview.data && (
              <div className="space-y-3">
                <label className="block space-y-1.5 text-body text-fg-1">
                  <span>{t("promptRewriter.result")}</span>
                  <Textarea
                    readOnly
                    className="min-h-[160px] font-mono text-xs"
                    value={JSON.stringify(preview.data.body, null, 2)}
                  />
                </label>
                <label className="block space-y-1.5 text-body text-fg-1">
                  <span>{t("promptRewriter.systemBefore")}</span>
                  <Textarea
                    readOnly
                    value={preview.data.systemBefore ?? ""}
                    placeholder={t("promptRewriter.notAvailable")}
                  />
                </label>
                <label className="block space-y-1.5 text-body text-fg-1">
                  <span>{t("promptRewriter.systemAfter")}</span>
                  <Textarea
                    readOnly
                    value={preview.data.systemAfter ?? ""}
                    placeholder={t("promptRewriter.notAvailable")}
                  />
                </label>
                <p className="text-caption text-fg-2">
                  {t("promptRewriter.fileIdentification")}:{" "}
                  {t(`promptRewriter.fileStates.${preview.data.fileStatus}`)}
                </p>
                <p className="text-caption text-fg-2">
                  {t("promptRewriter.systemApplication")}:{" "}
                  {t(
                    preview.data.systemApplied
                      ? "promptRewriter.applied"
                      : "promptRewriter.notApplied",
                  )}
                </p>
                <p className="text-caption text-fg-2">
                  {t("promptRewriter.userApplication")}:{" "}
                  {t(
                    preview.data.userApplied
                      ? "promptRewriter.applied"
                      : "promptRewriter.notApplied",
                  )}
                </p>
                {preview.data.reason && (
                  <p className="text-caption text-fg-2">
                    {t("promptRewriter.reason")}: {preview.data.reason}
                  </p>
                )}
              </div>
            )}
          </details>
          <ConfirmDialog
            isOpen={confirmation !== null}
            variant="info"
            title={t(
              confirmation?.kind === "reload"
                ? "promptRewriter.reloadTitle"
                : "promptRewriter.replaceTitle",
            )}
            message={t(
              confirmation?.kind === "reload"
                ? "promptRewriter.reloadMessage"
                : "promptRewriter.replaceMessage",
            )}
            confirmText={t(
              confirmation?.kind === "reload"
                ? "promptRewriter.discardReload"
                : "promptRewriter.replaceConfirm",
            )}
            cancelText={t("promptRewriter.keepEditing")}
            onConfirm={() => {
              if (confirmation) perform(confirmation);
              setConfirmation(null);
            }}
            onCancel={() => setConfirmation(null)}
          />
        </>
      )}
    </SettingsBlock>
  );
}
