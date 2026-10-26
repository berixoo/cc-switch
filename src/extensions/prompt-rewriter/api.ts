import { invoke } from "@tauri-apps/api/core";
import type { Config, Preview, PreviewInput, Route, Status } from "./types";

export const promptRewriterApi = {
  get: () => invoke<Status>("prompt_rewriter_get"),
  save: (config: Config) => invoke<Status>("prompt_rewriter_save", { config }),
  reload: () => invoke<Status>("prompt_rewriter_reload"),
  refreshSources: () => invoke<Status>("prompt_rewriter_refresh_sources"),
  capture: (route: Route) =>
    invoke<Status>("prompt_rewriter_capture", { route }),
  preview: (input: PreviewInput) =>
    invoke<Preview>("prompt_rewriter_preview", { ...input }),
};

export const promptRewriterKeys = {
  status: ["prompt-rewriter"] as const,
};
