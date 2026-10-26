import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { http, HttpResponse } from "msw";
import i18n from "i18next";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import ja from "@/i18n/locales/ja.json";
import tr from "@/i18n/locales/tr.json";
import { PromptRewriterPanel } from "@/extensions/prompt-rewriter/PromptRewriterPanel";
import {
  promptRewriterApi,
  promptRewriterKeys,
} from "@/extensions/prompt-rewriter/api";
import type {
  Config,
  Observation,
  Preview,
  PreviewInput,
  Route,
  RouteConfig,
  Source,
  Status,
} from "@/extensions/prompt-rewriter/types";
import { hasUnsavedChanges } from "@/lib/unsavedChanges";
import { server } from "../msw/server";

const ENDPOINT = "http://tauri.local";
const text = en.promptRewriter;
const makeRoute = (): RouteConfig => ({
  enabled: false,
  system: { enabled: false, replacement: null },
  userInstructions: { enabled: false, replacement: null, sourcePath: null },
});
const makeConfig = (): Config => ({
  version: 2,
  enabled: false,
  routes: {
    claudeCode: makeRoute(),
    claudeDesktop: makeRoute(),
  },
});
const makeSource = (route: Route): Source => ({
  path: "C:/home/.claude/CLAUDE.md",
  content: `${route} disk original\nsecond line`,
  state: "ready",
  error: null,
  loadedAt: 1792886400123,
});
const makeObservation = (patch: Partial<Observation> = {}): Observation => ({
  state: "notCaptured",
  system: null,
  providerId: null,
  model: null,
  format: null,
  requestId: null,
  capturedAt: null,
  fileStatus: "unknown",
  systemApplied: false,
  userApplied: false,
  error: null,
  ...patch,
});
const captured = (system = "Real ingress system\nsecond line"): Observation =>
  makeObservation({
    state: "captured",
    system,
    providerId: null,
    model: "original-client-model",
    format: "anthropic",
    requestId: "request-42",
    capturedAt: 1792886400456,
  });
const makeStatus = (config = makeConfig()): Status => ({
  available: true,
  config,
  path: "C:/test/extensions/prompt-rewriter.json",
  error: null,
  sources: {
    claudeCode: makeSource("claudeCode"),
    claudeDesktop: makeSource("claudeDesktop"),
  },
  observations: {
    claudeCode: makeObservation(),
    claudeDesktop: makeObservation(),
  },
});
const makePreview = (): Preview => ({
  body: {
    system: "rewritten",
    messages: [{ role: "user", content: "chat unchanged" }],
  },
  systemBefore: "original system",
  systemAfter: "rewritten",
  fileStatus: "unidentified",
  systemApplied: true,
  userApplied: false,
  reason: "No verified global instruction file boundary",
});

let runtime: Status;
let savedConfigs: Config[];
let previewInputs: PreviewInput[];
let captureRoutes: Route[];
let refreshBodies: unknown[];
let reloadCalls: number;
let getCalls: number;
const clients: QueryClient[] = [];

function mockGet(status: Status) {
  runtime = status;
}
function renderPanel(
  client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  }),
) {
  clients.push(client);
  return {
    ...render(
      <QueryClientProvider client={client}>
        <PromptRewriterPanel />
      </QueryClientProvider>,
    ),
    client,
  };
}
async function ready() {
  await screen.findByRole("switch", { name: text.enabled });
}
function editor(target: "system" | "userInstructions") {
  return screen.getByLabelText(text.replacements[target]);
}
function original(target: "system" | "userInstructions") {
  return screen.getByLabelText(text.originals[target]);
}
function edit(target: "system" | "userInstructions", value: string) {
  fireEvent.change(editor(target), { target: { value } });
}
function click(name: string) {
  fireEvent.click(screen.getByRole("button", { name }));
}
async function select(label: string, option: string) {
  fireEvent.keyDown(screen.getByRole("combobox", { name: label }), {
    key: "Enter",
  });
  fireEvent.click(await screen.findByRole("option", { name: option }));
}
function openPreview() {
  fireEvent.click(screen.getByText(text.preview));
}
async function previewDraft() {
  openPreview();
  click(text.runPreview);
  await screen.findByLabelText(text.result);
}

beforeEach(async () => {
  i18n.addResourceBundle("en", "translation", en, true, true);
  await i18n.changeLanguage("en");
  Element.prototype.scrollIntoView = vi.fn();
  runtime = makeStatus();
  savedConfigs = [];
  previewInputs = [];
  captureRoutes = [];
  refreshBodies = [];
  reloadCalls = 0;
  getCalls = 0;
  server.use(
    http.post(`${ENDPOINT}/prompt_rewriter_get`, () => {
      getCalls += 1;
      return HttpResponse.json(runtime);
    }),
    http.post(`${ENDPOINT}/prompt_rewriter_save`, async ({ request }) => {
      const { config } = (await request.json()) as { config: Config };
      savedConfigs.push(config);
      runtime = { ...runtime, config, error: null };
      return HttpResponse.json(runtime);
    }),
    http.post(`${ENDPOINT}/prompt_rewriter_reload`, () => {
      reloadCalls += 1;
      return HttpResponse.json(runtime);
    }),
    http.post(
      `${ENDPOINT}/prompt_rewriter_refresh_sources`,
      async ({ request }) => {
        refreshBodies.push(await request.json());
        return HttpResponse.json(runtime);
      },
    ),
    http.post(`${ENDPOINT}/prompt_rewriter_capture`, async ({ request }) => {
      const { route } = (await request.json()) as { route: Route };
      captureRoutes.push(route);
      runtime = {
        ...runtime,
        observations: {
          ...runtime.observations,
          [route]: makeObservation({ state: "waiting" }),
        },
      };
      return HttpResponse.json(runtime);
    }),
    http.post(`${ENDPOINT}/prompt_rewriter_preview`, async ({ request }) => {
      previewInputs.push((await request.json()) as PreviewInput);
      return HttpResponse.json(makePreview());
    }),
  );
});
afterEach(() => {
  clients.splice(0).forEach((client) => client.clear());
});

describe("PromptRewriterPanel full-text editing", () => {
  it("renders exactly two editable full-text editors, read-only originals and no matching/rule UI", async () => {
    renderPanel();
    await ready();
    expect(
      screen
        .getAllByRole("textbox")
        .filter(
          (node) =>
            node.tagName === "TEXTAREA" &&
            !node.hasAttribute("readonly") &&
            !node.closest("details"),
        ),
    ).toHaveLength(2);
    expect(original("system")).toHaveAttribute("readonly");
    expect(original("userInstructions")).toHaveAttribute("readonly");
    expect(editor("userInstructions")).toHaveValue(
      runtime.sources.claudeDesktop.content,
    );
    expect(
      screen.getByText(
        `${text.actualPath}: ${runtime.sources.claudeDesktop.path}`,
      ),
    ).toBeInTheDocument();
    expect(screen.getByText(text.sourceStates.ready)).toBeInTheDocument();
    expect(screen.getByText(text.loaded)).toBeInTheDocument();
    expect(screen.queryByText(text.saved)).not.toBeInTheDocument();
    expect(
      screen.getByText(text.preview).closest("details"),
    ).not.toHaveAttribute("open");
    expect(
      screen.queryByRole("button", {
        name: /add rule|move up|move down|delete rule/i,
      }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText(
        /rule id|find pattern|mode|processing stage|target/i,
      ),
    ).not.toBeInTheDocument();
    expect(hasUnsavedChanges()).toBe(false);
    expect(screen.getByRole("button", { name: text.save })).toBeDisabled();
  });

  it("edits both replacements and switches locally, then saves exact v2 config without changing originals", async () => {
    runtime.observations.claudeDesktop = captured();
    const initial = structuredClone(runtime);
    const { client } = renderPanel();
    await ready();
    expect(editor("system")).toHaveValue(
      initial.observations.claudeDesktop.system,
    );
    fireEvent.click(screen.getByRole("switch", { name: text.enabled }));
    fireEvent.click(screen.getByRole("switch", { name: text.routeEnabled }));
    fireEvent.click(
      screen.getByRole("switch", { name: text.enableTargets.system }),
    );
    fireEvent.click(
      screen.getByRole("switch", { name: text.enableTargets.userInstructions }),
    );
    edit("system", "system replacement\nfull body");
    edit("userInstructions", "file replacement\nfull body");
    expect(savedConfigs).toHaveLength(0);
    expect(original("system")).toHaveValue(
      initial.observations.claudeDesktop.system,
    );
    expect(original("userInstructions")).toHaveValue(
      initial.sources.claudeDesktop.content,
    );
    await waitFor(() => expect(hasUnsavedChanges()).toBe(true));
    const leave = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(leave);
    expect(leave.defaultPrevented).toBe(true);
    click(text.save);
    await screen.findByText(text.saved);
    const expected = makeConfig();
    expected.enabled = true;
    expected.routes.claudeDesktop = {
      enabled: true,
      system: { enabled: true, replacement: "system replacement\nfull body" },
      userInstructions: {
        enabled: true,
        replacement: "file replacement\nfull body",
        sourcePath: null,
      },
    };
    expect(savedConfigs).toEqual([expected]);
    expect(client.getQueryData(promptRewriterKeys.status)).toEqual({
      ...initial,
      config: expected,
    });
    expect(screen.getByRole("button", { name: text.save })).toBeDisabled();
    await waitFor(() => expect(hasUnsavedChanges()).toBe(false));
  });

  it("never uses saved replacements as captured system or disk originals", async () => {
    runtime.config.routes.claudeDesktop.system.replacement =
      "saved system replacement";
    runtime.config.routes.claudeDesktop.userInstructions.replacement =
      "saved file replacement";
    renderPanel();
    await ready();
    expect(original("system")).toHaveValue("");
    expect(original("system")).toHaveAttribute(
      "placeholder",
      text.noSystemOriginal,
    );
    expect(editor("system")).toHaveValue("saved system replacement");
    expect(original("userInstructions")).toHaveValue(
      runtime.sources.claudeDesktop.content,
    );
    expect(editor("userInstructions")).toHaveValue("saved file replacement");
    const section = within(
      screen.getByRole("region", { name: text.editors.system }),
    );
    expect(
      section.getByRole("button", { name: text.loadOriginal }),
    ).toBeDisabled();
    expect(
      section.getByText(text.captureStates.notCaptured),
    ).toBeInTheDocument();
    expect(screen.queryByText(text.emptyOriginal)).not.toBeInTheDocument();
  });

  it("distinguishes successfully empty originals, null inheritance and an explicit empty replacement", async () => {
    runtime.observations.claudeDesktop = captured("");
    runtime.sources.claudeDesktop.content = "";
    renderPanel();
    await ready();
    expect(
      within(
        screen.getByRole("region", { name: text.editors.system }),
      ).getByText(text.emptyOriginal),
    ).toBeInTheDocument();
    expect(
      within(
        screen.getByRole("region", { name: text.editors.userInstructions }),
      ).getByText(text.emptyFileOriginal),
    ).toBeInTheDocument();
    expect(screen.getAllByText(text.emptyOriginal)).toHaveLength(1);
    expect(screen.getAllByText(text.inherited)).toHaveLength(2);
    expect(
      screen.getAllByRole("button", { name: text.loadOriginal })[0],
    ).toBeEnabled();
    edit("system", "temporary");
    edit("system", "");
    expect(screen.getByText(text.emptyReplacement)).toBeInTheDocument();
    click(text.save);
    await screen.findByText(text.saved);
    expect(savedConfigs[0].routes.claudeDesktop.system.replacement).toBe("");
    expect(
      savedConfigs[0].routes.claudeDesktop.userInstructions.replacement,
    ).toBeNull();
  });

  it.each(["missing", "error"] as const)(
    "shows %s source state separately from an empty file",
    async (state) => {
      runtime.sources.claudeDesktop = {
        ...makeSource("claudeDesktop"),
        state,
        content: null,
        error: state === "error" ? "File is not UTF-8" : null,
      };
      renderPanel();
      await ready();
      expect(screen.getByText(text.sourceStates[state])).toBeInTheDocument();
      expect(original("userInstructions")).toHaveAttribute(
        "placeholder",
        text.noFileOriginal,
      );
      expect(screen.queryByText(text.emptyOriginal)).not.toBeInTheDocument();
      expect(
        screen.queryByText(text.emptyFileOriginal),
      ).not.toBeInTheDocument();
      expect(
        within(
          screen.getByRole("region", { name: text.editors.userInstructions }),
        ).getByRole("button", { name: text.loadOriginal }),
      ).toBeDisabled();
      if (state === "error")
        expect(screen.getByRole("alert")).toHaveTextContent(
          "File is not UTF-8",
        );
    },
  );

  it("offers only Claude Code CLI and Claude Desktop routes, never Codex", async () => {
    renderPanel();
    await ready();
    fireEvent.keyDown(screen.getByRole("combobox", { name: text.route }), {
      key: "Enter",
    });
    const options = await screen.findAllByRole("option");
    expect(options).toHaveLength(2);
    expect(options.map((option) => option.textContent)).toEqual([
      text.routes.claudeCode,
      text.routes.claudeDesktop,
    ]);
    expect(
      screen.queryByRole("option", { name: /codex/i }),
    ).not.toBeInTheDocument();
  });

  it("keeps CLI and Desktop drafts independent, including enabled targets and source paths", async () => {
    runtime.config.routes.claudeDesktop.enabled = true;
    runtime.config.routes.claudeDesktop.system = {
      enabled: true,
      replacement: "desktop only",
    };
    renderPanel();
    await ready();
    edit("system", "desktop draft");
    edit("userInstructions", "desktop file");
    fireEvent.change(screen.getByLabelText(text.sourcePath), {
      target: { value: "D:/desktop/custom.md" },
    });
    const desktop: RouteConfig = {
      enabled: true,
      system: { enabled: true, replacement: "desktop draft" },
      userInstructions: {
        enabled: false,
        replacement: "desktop file",
        sourcePath: "D:/desktop/custom.md",
      },
    };
    await select(text.route, text.routes.claudeCode);
    expect(
      screen.getByRole("switch", { name: text.routeEnabled }),
    ).not.toBeChecked();
    expect(editor("system")).toHaveValue("");
    expect(original("userInstructions")).toHaveValue(
      runtime.sources.claudeCode.content,
    );
    fireEvent.click(screen.getByRole("switch", { name: text.routeEnabled }));
    fireEvent.click(
      screen.getByRole("switch", { name: text.enableTargets.system }),
    );
    fireEvent.click(
      screen.getByRole("switch", { name: text.enableTargets.userInstructions }),
    );
    edit("system", "cli system");
    edit("userInstructions", "cli file");
    fireEvent.change(screen.getByLabelText(text.sourcePath), {
      target: { value: "D:/cli/CLAUDE.md" },
    });
    await select(text.route, text.routes.claudeDesktop);
    expect(editor("system")).toHaveValue("desktop draft");
    expect(editor("userInstructions")).toHaveValue("desktop file");
    expect(screen.getByLabelText(text.sourcePath)).toHaveValue(
      "D:/desktop/custom.md",
    );
    await select(text.route, text.routes.claudeCode);
    expect(editor("system")).toHaveValue("cli system");
    expect(editor("userInstructions")).toHaveValue("cli file");
    click(text.save);
    await screen.findByText(text.saved);
    expect(savedConfigs[0].routes).toEqual({
      claudeDesktop: desktop,
      claudeCode: {
        enabled: true,
        system: { enabled: true, replacement: "cli system" },
        userInstructions: {
          enabled: true,
          replacement: "cli file",
          sourcePath: "D:/cli/CLAUDE.md",
        },
      },
    });
  });

  it("captures CLI ingress independently and polls it while viewing Desktop without erasing drafts", async () => {
    runtime.observations.claudeDesktop = captured("desktop original");
    const desktop = structuredClone(runtime.observations.claudeDesktop);
    renderPanel();
    await ready();
    await select(text.route, text.routes.claudeCode);
    edit("system", "keep CLI draft");
    click(text.capture);
    await screen.findByText(text.captureStates.waiting);
    expect(captureRoutes).toEqual(["claudeCode"]);
    await select(text.route, text.routes.claudeDesktop);
    expect(original("system")).toHaveValue("desktop original");
    runtime = {
      ...runtime,
      observations: {
        ...runtime.observations,
        claudeCode: captured("cli original"),
      },
    };
    const beforePoll = getCalls;
    await waitFor(() => expect(getCalls).toBeGreaterThan(beforePoll), {
      timeout: 3500,
    });
    await select(text.route, text.routes.claudeCode);
    await waitFor(() => expect(original("system")).toHaveValue("cli original"));
    expect(editor("system")).toHaveValue("keep CLI draft");
    expect(screen.getByText(text.providerNotSelected)).toBeInTheDocument();
    expect(runtime.observations.claudeDesktop).toEqual(desktop);
    expect(savedConfigs).toHaveLength(0);
  });

  it("refreshes sources using saved paths without sending config or overwriting dirty text and fallback originals", async () => {
    const { client } = renderPanel();
    await ready();
    edit("system", "unsaved system");
    fireEvent.change(screen.getByLabelText(text.sourcePath), {
      target: { value: "D:/not-saved.md" },
    });
    const previousFile = runtime.sources.claudeDesktop.content;
    runtime = {
      ...runtime,
      sources: {
        ...runtime.sources,
        claudeDesktop: {
          ...runtime.sources.claudeDesktop,
          content: "external disk update",
          loadedAt: 1792886500000,
        },
      },
    };
    click(text.refreshSources);
    await waitFor(() =>
      expect(original("userInstructions")).toHaveValue("external disk update"),
    );
    expect(refreshBodies).toEqual([{}]);
    expect(client.getQueryData(promptRewriterKeys.status)).toEqual(runtime);
    expect(editor("userInstructions")).toHaveValue(previousFile);
    expect(editor("system")).toHaveValue("unsaved system");
    expect(screen.getByLabelText(text.sourcePath)).toHaveValue(
      "D:/not-saved.md",
    );
    expect(screen.getByText(text.dirty)).toBeInTheDocument();
    expect(savedConfigs).toHaveLength(0);
    const file = within(
      screen.getByRole("region", { name: text.editors.userInstructions }),
    );
    fireEvent.click(file.getByRole("button", { name: text.loadOriginal }));
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      text.replaceTitle,
    );
    click(text.keepEditing);
    expect(editor("userInstructions")).toHaveValue(previousFile);
    fireEvent.click(file.getByRole("button", { name: text.loadOriginal }));
    click(text.replaceConfirm);
    expect(editor("userInstructions")).toHaveValue("external disk update");
    expect(editor("system")).toHaveValue("unsaved system");
  });

  it("clears a custom source path to null without making any filesystem call", async () => {
    runtime.config.routes.claudeDesktop.userInstructions.sourcePath =
      "D:/custom.md";
    renderPanel();
    await ready();
    fireEvent.change(screen.getByLabelText(text.sourcePath), {
      target: { value: "" },
    });
    expect(refreshBodies).toHaveLength(0);
    click(text.save);
    await screen.findByText(text.saved);
    expect(
      savedConfigs[0].routes.claudeDesktop.userInstructions.sourcePath,
    ).toBeNull();
  });

  it("arms real capture with only route, polls only while waiting, and preserves dirty replacement on observation", async () => {
    const { client } = renderPanel();
    await ready();
    edit("system", "retain this draft");
    click(text.capture);
    await screen.findByText(text.captureStates.waiting);
    expect(captureRoutes).toEqual(["claudeDesktop"]);
    expect(screen.getByRole("button", { name: text.capture })).toBeDisabled();
    expect(original("system")).toHaveValue("");
    runtime = {
      ...runtime,
      observations: { ...runtime.observations, claudeDesktop: captured() },
    };
    await waitFor(
      () =>
        expect(original("system")).toHaveValue(
          runtime.observations.claudeDesktop.system,
        ),
      { timeout: 3500 },
    );
    expect(getCalls).toBeGreaterThan(1);
    expect(editor("system")).toHaveValue("retain this draft");
    expect(screen.getByText(text.providerNotSelected)).toBeInTheDocument();
    expect(screen.getByText("original-client-model")).toBeInTheDocument();
    expect(screen.getByText("request-42")).toBeInTheDocument();
    expect(
      screen.getByText(new Date(1792886400456).toLocaleString()),
    ).toBeInTheDocument();
    expect(client.getQueryData(promptRewriterKeys.status)).toEqual(runtime);
    const stoppedAt = getCalls;
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect(getCalls).toBe(stoppedAt);
    expect(screen.getByRole("button", { name: text.capture })).toBeEnabled();
    expect(savedConfigs).toHaveLength(0);
  });

  it("arms the selected route even when all replacement switches are off", async () => {
    renderPanel();
    await ready();
    await select(text.route, text.routes.claudeCode);
    click(text.capture);
    await screen.findByText(text.captureStates.waiting);
    expect(captureRoutes).toEqual(["claudeCode"]);
    expect(
      screen.getByRole("switch", { name: text.enabled }),
    ).not.toBeChecked();
    expect(screen.queryByText(text.dirty)).not.toBeInTheDocument();
    expect(hasUnsavedChanges()).toBe(false);
  });

  it("loads captured original explicitly and restores inheritance by disabling a target without saving", async () => {
    runtime.observations.claudeDesktop = captured();
    runtime.config.routes.claudeDesktop.system = {
      enabled: true,
      replacement: "old saved replacement",
    };
    renderPanel();
    await ready();
    const system = within(
      screen.getByRole("region", { name: text.editors.system }),
    );
    fireEvent.click(system.getByRole("button", { name: text.loadOriginal }));
    expect(editor("system")).toHaveValue(
      runtime.observations.claudeDesktop.system,
    );
    expect(savedConfigs).toHaveLength(0);
    fireEvent.click(system.getByRole("button", { name: text.resetTarget }));
    await screen.findByRole("dialog");
    click(text.replaceConfirm);
    expect(
      screen.getByRole("switch", { name: text.enableTargets.system }),
    ).not.toBeChecked();
    expect(editor("system")).toHaveValue(
      runtime.observations.claudeDesktop.system,
    );
    click(text.save);
    await screen.findByText(text.saved);
    expect(savedConfigs[0].routes.claudeDesktop.system).toEqual({
      enabled: false,
      replacement: null,
    });
  });
});

describe("PromptRewriterPanel cache and failure behavior", () => {
  it("hides the whole panel and disables polling when the feature is unavailable", async () => {
    runtime.available = false;
    runtime.observations.claudeDesktop.state = "waiting";
    const { container, client } = renderPanel();
    await waitFor(() =>
      expect(client.getQueryState(promptRewriterKeys.status)?.status).toBe(
        "success",
      ),
    );
    expect(container).toBeEmptyDOMElement();
    expect(getCalls).toBe(1);
    expect(savedConfigs).toHaveLength(0);
  });

  it("uses default MSW feature-off v2 status with all source and observation fields", async () => {
    server.resetHandlers();
    const result = await promptRewriterApi.get();
    expect(result.available).toBe(false);
    expect(result.config).toEqual(makeConfig());
    expect(result.sources.claudeDesktop).toEqual({
      path: null,
      content: null,
      state: "missing",
      error: null,
      loadedAt: null,
    });
    expect(result.sources.claudeCode).toEqual(result.sources.claudeDesktop);
    expect(result.observations.claudeCode).toEqual(makeObservation());
    expect(result.observations.claudeDesktop).toEqual(makeObservation());
    expect(Object.keys(result.sources).sort()).toEqual([
      "claudeCode",
      "claudeDesktop",
    ]);
    expect(Object.keys(result.observations).sort()).toEqual([
      "claudeCode",
      "claudeDesktop",
    ]);
    expect(await promptRewriterApi.refreshSources()).toEqual(result);
    expect(await promptRewriterApi.capture("claudeCode")).toEqual(result);
    expect(await promptRewriterApi.capture("claudeDesktop")).toEqual(result);
  });

  it("keeps a failed get retryable and never claims save after retry", async () => {
    server.use(
      http.post(
        `${ENDPOINT}/prompt_rewriter_get`,
        () => new HttpResponse("status unavailable", { status: 500 }),
      ),
    );
    renderPanel();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "status unavailable",
    );
    expect(
      screen.queryByRole("button", { name: text.save }),
    ).not.toBeInTheDocument();
    server.use(
      http.post(`${ENDPOINT}/prompt_rewriter_get`, () =>
        HttpResponse.json(runtime),
      ),
    );
    click(text.retry);
    await ready();
    expect(screen.getByText(text.loaded)).toBeInTheDocument();
    expect(screen.queryByText(text.saved)).not.toBeInTheDocument();
  });

  it("preserves startup errors and enables explicit repair save", async () => {
    runtime.error = "unsupported version 1; file preserved";
    renderPanel();
    await ready();
    expect(screen.getByRole("alert")).toHaveTextContent(runtime.error);
    expect(screen.getByRole("button", { name: text.save })).toBeEnabled();
    expect(screen.queryByText(text.saved)).not.toBeInTheDocument();
    click(text.save);
    await screen.findByText(text.saved);
    expect(savedConfigs).toEqual([makeConfig()]);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it.each(["domain", "transport"])(
    "retains dirty config on %s save failure without a saved indication",
    async (failure) => {
      const failed = { ...runtime, error: "write failed" };
      server.use(
        http.post(`${ENDPOINT}/prompt_rewriter_save`, () =>
          failure === "domain"
            ? HttpResponse.json(failed)
            : new HttpResponse("write failed", { status: 500 }),
        ),
      );
      const { client } = renderPanel();
      await ready();
      edit("system", "keep draft");
      click(text.save);
      expect(await screen.findByRole("alert")).toHaveTextContent(
        "write failed",
      );
      expect(editor("system")).toHaveValue("keep draft");
      expect(screen.getByText(text.dirty)).toBeInTheDocument();
      expect(screen.queryByText(text.saved)).not.toBeInTheDocument();
      if (failure === "domain")
        expect(client.getQueryData(promptRewriterKeys.status)).toEqual(failed);
    },
  );

  it("follows clean cached config on initial get and refreshed observations without becoming dirty", async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    client.setQueryData(promptRewriterKeys.status, runtime);
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const refreshed = makeStatus();
    refreshed.config.enabled = true;
    refreshed.config.routes.claudeDesktop.system.replacement =
      "server replacement";
    refreshed.sources.claudeDesktop.content = "new disk content";
    server.use(
      http.post(`${ENDPOINT}/prompt_rewriter_get`, async () => {
        await gate;
        return HttpResponse.json(refreshed);
      }),
    );
    renderPanel(client);
    await ready();
    expect(editor("system")).toHaveValue("");
    release();
    await waitFor(() =>
      expect(editor("system")).toHaveValue("server replacement"),
    );
    expect(editor("userInstructions")).toHaveValue("new disk content");
    expect(screen.getByRole("switch", { name: text.enabled })).toBeChecked();
    expect(screen.queryByText(text.dirty)).not.toBeInTheDocument();
    expect(hasUnsavedChanges()).toBe(false);
  });

  it("keeps dirty config through get refresh and stops calling refreshed query results saved", async () => {
    const { client } = renderPanel();
    await ready();
    edit("system", "saved text");
    click(text.save);
    await screen.findByText(text.saved);
    await act(async () => {
      await client.refetchQueries({ queryKey: promptRewriterKeys.status });
    });
    await waitFor(() =>
      expect(screen.queryByText(text.saved)).not.toBeInTheDocument(),
    );
    expect(screen.getByText(text.loaded)).toBeInTheDocument();
    edit("system", "unsaved text");
    const next = makeStatus();
    next.config.enabled = true;
    next.observations.claudeDesktop = captured("new observation");
    mockGet(next);
    await act(async () => {
      await client.refetchQueries({ queryKey: promptRewriterKeys.status });
    });
    expect(editor("system")).toHaveValue("unsaved text");
    await waitFor(() =>
      expect(original("system")).toHaveValue("new observation"),
    );
    expect(
      screen.getByRole("switch", { name: text.enabled }),
    ).not.toBeChecked();
    expect(savedConfigs).toHaveLength(1);
  });

  it("guards dirty reload, keeps draft on cancel and transport error, then clears it only on success", async () => {
    const { client } = renderPanel();
    await ready();
    edit("system", "keep on reload failure");
    click(text.reload);
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      text.reloadTitle,
    );
    expect(reloadCalls).toBe(0);
    click(text.keepEditing);
    expect(editor("system")).toHaveValue("keep on reload failure");
    server.use(
      http.post(
        `${ENDPOINT}/prompt_rewriter_reload`,
        () => new HttpResponse("invalid file", { status: 500 }),
      ),
    );
    click(text.reload);
    click(text.discardReload);
    expect(await screen.findByRole("alert")).toHaveTextContent("invalid file");
    expect(editor("system")).toHaveValue("keep on reload failure");
    expect(screen.getByText(text.dirty)).toBeInTheDocument();
    const reloaded = makeStatus();
    reloaded.config.routes.claudeDesktop.system.replacement =
      "disk replacement";
    server.use(
      http.post(`${ENDPOINT}/prompt_rewriter_reload`, () =>
        HttpResponse.json(reloaded),
      ),
    );
    click(text.reload);
    click(text.discardReload);
    await waitFor(() =>
      expect(editor("system")).toHaveValue("disk replacement"),
    );
    expect(client.getQueryData(promptRewriterKeys.status)).toEqual(reloaded);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.queryByText(text.saved)).not.toBeInTheDocument();
    await waitFor(() => expect(hasUnsavedChanges()).toBe(false));
  });

  it("synchronizes same-path domain reload errors and retains active runtime config", async () => {
    runtime.config.enabled = true;
    const failed = { ...runtime, error: "invalid config on same path" };
    server.use(
      http.post(`${ENDPOINT}/prompt_rewriter_reload`, () =>
        HttpResponse.json(failed),
      ),
    );
    const { client } = renderPanel();
    await ready();
    click(text.reload);
    expect(await screen.findByRole("alert")).toHaveTextContent(failed.error);
    expect(client.getQueryData(promptRewriterKeys.status)).toEqual(failed);
    expect(screen.getByRole("switch", { name: text.enabled })).toBeChecked();
    expect(screen.getByRole("button", { name: text.save })).toBeEnabled();
    expect(screen.queryByText(text.dirty)).not.toBeInTheDocument();
    click(text.save);
    await screen.findByText(text.saved);
    expect(savedConfigs[0].enabled).toBe(true);
  });

  it.each([false, true])(
    "publishes cross-path reload domain errors while retaining only genuinely dirty drafts (dirty=%s)",
    async (dirty) => {
      runtime.config.enabled = true;
      const failed = {
        ...makeStatus(),
        path: "D:/new-config/extensions/prompt-rewriter.json",
        error: "new directory file invalid",
      };
      server.use(
        http.post(`${ENDPOINT}/prompt_rewriter_reload`, () =>
          HttpResponse.json(failed),
        ),
      );
      const { client } = renderPanel();
      await ready();
      if (dirty) edit("system", "retain genuinely edited text");
      click(text.reload);
      if (dirty) click(text.discardReload);
      await screen.findByText(failed.path);
      expect(client.getQueryData(promptRewriterKeys.status)).toEqual(failed);
      expect(screen.getByRole("alert")).toHaveTextContent(failed.error);
      expect(
        screen.getByRole("switch", { name: text.enabled }),
      ).toHaveAttribute("aria-checked", dirty ? "true" : "false");
      expect(editor("system")).toHaveValue(
        dirty ? "retain genuinely edited text" : "",
      );
      expect(screen.getByRole("button", { name: text.save })).toBeEnabled();
      expect(hasUnsavedChanges()).toBe(dirty);
    },
  );

  it.each(["refresh_sources", "capture"])(
    "reports %s transport failures without changing drafts",
    async (command) => {
      server.use(
        http.post(
          `${ENDPOINT}/prompt_rewriter_${command}`,
          () => new HttpResponse("operation failed", { status: 500 }),
        ),
      );
      renderPanel();
      await ready();
      edit("system", "preserved");
      click(command === "capture" ? text.capture : text.refreshSources);
      expect(await screen.findByRole("alert")).toHaveTextContent(
        "operation failed",
      );
      expect(editor("system")).toHaveValue("preserved");
      expect(savedConfigs).toHaveLength(0);
    },
  );

  it("shows observation errors without inventing system text", async () => {
    runtime.observations.claudeDesktop = makeObservation({
      state: "error",
      error: "capture exceeded size limit",
    });
    renderPanel();
    await ready();
    expect(screen.getByText(text.captureStates.error)).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "capture exceeded size limit",
    );
    expect(original("system")).toHaveValue("");
    expect(editor("system")).toHaveValue("");
    expect(screen.queryByText(text.emptyOriginal)).not.toBeInTheDocument();
  });
});

describe("PromptRewriterPanel empty-source status", () => {
  it.each([
    ["en", en],
    ["zh", zh],
    ["zh-TW", zhTW],
    ["ja", ja],
    ["tr", tr],
  ] as const)(
    "distinguishes injected instructions from an identified original file in %s",
    async (language, resources) => {
      const labels = resources.promptRewriter;
      i18n.addResourceBundle(language, "translation", resources, true, true);
      await i18n.changeLanguage(language);
      runtime.sources.claudeDesktop.content = "";
      runtime.observations.claudeDesktop = makeObservation({
        ...captured(""),
        fileStatus: "injected",
        userApplied: true,
      });
      const { client } = renderPanel();
      await screen.findByRole("switch", { name: labels.enabled });
      const file = within(
        screen.getByRole("region", { name: labels.editors.userInstructions }),
      );
      const system = within(
        screen.getByRole("region", { name: labels.editors.system }),
      );
      expect(
        file.getByText(
          `${labels.fileIdentification}: ${labels.fileStates.injected}`,
        ),
      ).toBeInTheDocument();
      expect(file.getByText(labels.emptyFileOriginal)).toBeInTheDocument();
      expect(system.getByText(labels.emptyOriginal)).toBeInTheDocument();
      expect(
        system.queryByText(labels.emptyFileOriginal),
      ).not.toBeInTheDocument();
      expect(labels.fileStates.injected).not.toBe(labels.fileStates.identified);
      expect(
        screen.queryByText(/promptRewriter\.fileStates\.injected/),
      ).not.toBeInTheDocument();
      expect(
        screen.getByLabelText(labels.originals.userInstructions),
      ).toHaveValue("");
      runtime = {
        ...runtime,
        observations: {
          ...runtime.observations,
          claudeDesktop: {
            ...runtime.observations.claudeDesktop,
            fileStatus: "identified",
          },
        },
      };
      await act(async () => {
        await client.refetchQueries({ queryKey: promptRewriterKeys.status });
      });
      expect(
        await file.findByText(
          `${labels.fileIdentification}: ${labels.fileStates.identified}`,
        ),
      ).toBeInTheDocument();
      expect(
        file.queryByText(
          `${labels.fileIdentification}: ${labels.fileStates.injected}`,
        ),
      ).not.toBeInTheDocument();
      expect(savedConfigs).toHaveLength(0);
      expect(hasUnsavedChanges()).toBe(false);
    },
  );

  it.each(["claudeCode", "claudeDesktop"] as const)(
    "renders the injected preview response for %s without claiming an identified file or changing originals",
    async (route) => {
      const replacement = "request-only user instructions";
      const wrapped = `<cc-switch-user-instructions>\n${replacement}\n</cc-switch-user-instructions>`;
      const chat = { role: "user", content: "chat unchanged" };
      const body = { system: "original system", messages: [chat] };
      const rewritten: Preview["body"] = {
        system: "rewritten",
        messages: [
          { role: "user", content: [{ type: "text", text: wrapped }] },
          chat,
        ],
      };
      const result: Preview = {
        ...makePreview(),
        body: rewritten,
        fileStatus: "injected",
        userApplied: true,
        reason: null,
      };
      runtime.sources[route].content = "";
      runtime.config.routes[route].userInstructions = {
        enabled: true,
        replacement,
        sourcePath: null,
      };
      server.use(
        http.post(
          `${ENDPOINT}/prompt_rewriter_preview`,
          async ({ request }) => {
            previewInputs.push((await request.json()) as PreviewInput);
            return HttpResponse.json(result);
          },
        ),
      );
      renderPanel();
      await ready();
      if (route !== "claudeDesktop")
        await select(text.route, text.routes[route]);
      openPreview();
      fireEvent.change(screen.getByLabelText(text.body), {
        target: { value: JSON.stringify(body) },
      });
      click(text.runPreview);
      expect(await screen.findByLabelText(text.result)).toHaveValue(
        JSON.stringify(rewritten, null, 2),
      );
      expect(
        screen.getByText(
          `${text.fileIdentification}: ${text.fileStates.injected}`,
        ),
      ).toBeInTheDocument();
      expect(
        screen.queryByText(
          `${text.fileIdentification}: ${text.fileStates.identified}`,
        ),
      ).not.toBeInTheDocument();
      expect(
        screen.getByText(`${text.userApplication}: ${text.applied}`),
      ).toBeInTheDocument();
      expect(screen.getByLabelText(text.body)).toHaveValue(
        JSON.stringify(body),
      );
      expect(original("userInstructions")).toHaveValue("");
      expect(original("system")).toHaveValue("");
      expect(previewInputs[0]).toMatchObject({
        route,
        format: "anthropic",
        body,
        config: {
          routes: {
            [route]: { userInstructions: { enabled: true, replacement } },
          },
        },
      });
      expect(savedConfigs).toHaveLength(0);
      expect(captureRoutes).toHaveLength(0);
    },
  );

  it("shows the backend empty-source and empty-replacement reason without claiming injection", async () => {
    runtime.sources.claudeDesktop.content = "";
    runtime.config.routes.claudeDesktop.userInstructions = {
      enabled: true,
      replacement: "",
      sourcePath: null,
    };
    const result: Preview = {
      ...makePreview(),
      reason: "Source and replacement are empty; no instruction message added",
    };
    server.use(
      http.post(`${ENDPOINT}/prompt_rewriter_preview`, () =>
        HttpResponse.json(result),
      ),
    );
    renderPanel();
    await ready();
    await previewDraft();
    expect(
      screen.getByText(`${text.reason}: ${result.reason}`),
    ).toBeInTheDocument();
    expect(
      screen.getByText(`${text.userApplication}: ${text.notApplied}`),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(
        `${text.fileIdentification}: ${text.fileStates.injected}`,
      ),
    ).not.toBeInTheDocument();
    expect(screen.getByText(text.emptyReplacement)).toBeInTheDocument();
    expect(original("userInstructions")).toHaveValue("");
    expect(savedConfigs).toHaveLength(0);
  });
});

describe("PromptRewriterPanel advanced preview", () => {
  it("uses Anthropic ingress for both routes and clears stale results when switching", async () => {
    renderPanel();
    await ready();
    openPreview();
    for (const [route, format] of [
      ["claudeCode", "anthropic"],
      ["claudeDesktop", "anthropic"],
    ] as const) {
      await select(text.route, text.routes[route]);
      expect(screen.queryByLabelText(text.result)).not.toBeInTheDocument();
      expect(
        screen.queryByRole("combobox", { name: text.format }),
      ).not.toBeInTheDocument();
      expect(
        screen.getByText(`${text.format}: ${text.formats[format]}`),
      ).toBeInTheDocument();
      click(text.runPreview);
      await screen.findByLabelText(text.result);
      expect(previewInputs[previewInputs.length - 1]).toMatchObject({
        route,
        format,
      });
    }
    expect(savedConfigs).toHaveLength(0);
  });
  it("previews the unsaved full-text config without target, reports unidentified file and preserves chat JSON", async () => {
    renderPanel();
    await ready();
    edit("system", "draft system");
    edit("userInstructions", "draft file");
    openPreview();
    expect(
      screen.queryByRole("combobox", { name: text.format }),
    ).not.toBeInTheDocument();
    const body = {
      system: "original system",
      messages: [{ role: "user", content: "chat unchanged" }],
    };
    fireEvent.change(screen.getByLabelText(text.body), {
      target: { value: JSON.stringify(body) },
    });
    click(text.runPreview);
    expect(await screen.findByLabelText(text.result)).toHaveValue(
      JSON.stringify(makePreview().body, null, 2),
    );
    expect(screen.getByLabelText(text.systemBefore)).toHaveValue(
      "original system",
    );
    expect(screen.getByLabelText(text.systemAfter)).toHaveValue("rewritten");
    expect(
      screen.getByText(
        `${text.fileIdentification}: ${text.fileStates.unidentified}`,
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(`${text.userApplication}: ${text.notApplied}`),
    ).toBeInTheDocument();
    expect(
      screen.getByText(`${text.reason}: ${makePreview().reason}`),
    ).toBeInTheDocument();
    const expected = makeConfig();
    expected.routes.claudeDesktop.system.replacement = "draft system";
    expected.routes.claudeDesktop.userInstructions.replacement = "draft file";
    expect(previewInputs).toEqual([
      { config: expected, route: "claudeDesktop", format: "anthropic", body },
    ]);
    expect(previewInputs[0]).not.toHaveProperty("target");
    expect(
      screen.queryByRole("combobox", { name: /target|processing stage/i }),
    ).not.toBeInTheDocument();
    expect(savedConfigs).toHaveLength(0);
    expect(captureRoutes).toHaveLength(0);
    expect(original("system")).toHaveValue("");
  });

  it.each(["reload", "reset", "edit", "body", "route"])(
    "clears a previous preview on %s",
    async (action) => {
      runtime.config.routes.claudeDesktop.system = {
        enabled: true,
        replacement: "saved replacement",
      };
      renderPanel();
      await ready();
      await previewDraft();
      if (action === "reload") click(text.reload);
      if (action === "reset")
        fireEvent.click(
          within(
            screen.getByRole("region", { name: text.editors.system }),
          ).getByRole("button", { name: text.resetTarget }),
        );
      if (action === "edit") edit("system", "new draft");
      if (action === "body")
        fireEvent.change(screen.getByLabelText(text.body), {
          target: { value: "{} " },
        });
      if (action === "route") await select(text.route, text.routes.claudeCode);
      await waitFor(() =>
        expect(screen.queryByLabelText(text.result)).not.toBeInTheDocument(),
      );
    },
  );

  it("rejects invalid JSON locally and clears validation error after reload", async () => {
    renderPanel();
    await ready();
    openPreview();
    fireEvent.change(screen.getByLabelText(text.body), {
      target: { value: "{" },
    });
    click(text.runPreview);
    expect(await screen.findByRole("alert")).toHaveTextContent("Invalid JSON");
    expect(screen.getByLabelText(text.body)).toHaveAttribute(
      "aria-invalid",
      "true",
    );
    expect(previewInputs).toHaveLength(0);
    click(text.reload);
    await waitFor(() =>
      expect(screen.queryByRole("alert")).not.toBeInTheDocument(),
    );
    expect(screen.getByLabelText(text.body)).toHaveAttribute(
      "aria-invalid",
      "false",
    );
  });

  it("preserves JSON and draft on a backend preview failure without saving", async () => {
    server.use(
      http.post(
        `${ENDPOINT}/prompt_rewriter_preview`,
        () => new HttpResponse("engine unavailable", { status: 500 }),
      ),
    );
    renderPanel();
    await ready();
    edit("system", "my draft");
    openPreview();
    fireEvent.change(screen.getByLabelText(text.body), {
      target: { value: '{"system":"original"}' },
    });
    click(text.runPreview);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "engine unavailable",
    );
    expect(screen.getByLabelText(text.body)).toHaveValue(
      '{"system":"original"}',
    );
    expect(editor("system")).toHaveValue("my draft");
    expect(savedConfigs).toHaveLength(0);
  });
});
