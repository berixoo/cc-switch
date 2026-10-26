export type Route = "claudeCode" | "claudeDesktop";
export type WireFormat = "anthropic" | "responses" | "chat" | "gemini";
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export interface ReplacementConfig {
  enabled: boolean;
  replacement: string | null;
}

export interface RouteConfig {
  enabled: boolean;
  system: ReplacementConfig;
  userInstructions: ReplacementConfig & { sourcePath: string | null };
}

export interface Config {
  version: 2;
  enabled: boolean;
  routes: Record<Route, RouteConfig>;
}

export interface Source {
  path: string | null;
  content: string | null;
  state: "ready" | "missing" | "error";
  error: string | null;
  loadedAt: number | null;
}

export type FileStatus =
  | "unknown"
  | "unidentified"
  | "ambiguous"
  | "identified"
  | "injected";

export interface Observation {
  state: "notCaptured" | "waiting" | "captured" | "error";
  system: string | null;
  providerId: string | null;
  model: string | null;
  format: WireFormat | null;
  requestId: string | null;
  capturedAt: number | null;
  fileStatus: FileStatus;
  systemApplied: boolean;
  userApplied: boolean;
  error: string | null;
}

export interface Status {
  available: boolean;
  config: Config;
  path: string;
  error: string | null;
  sources: Record<Route, Source>;
  observations: Record<Route, Observation>;
}

export interface PreviewInput {
  config: Config;
  route: Route;
  format: "anthropic";
  body: JsonValue;
}

export interface Preview {
  body: JsonValue;
  systemBefore: string | null;
  systemAfter: string | null;
  fileStatus: FileStatus;
  systemApplied: boolean;
  userApplied: boolean;
  reason: string | null;
}
