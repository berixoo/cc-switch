import { render, screen, fireEvent } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import "@testing-library/jest-dom";
import { AboutSection } from "@/components/settings/AboutSection";

const updateState = vi.hoisted(() => ({
  hasUpdate: true,
  updateInfo: {
    currentVersion: "4.0.20",
    availableVersion: "4.0.21",
  } as { currentVersion: string; availableVersion: string } | null,
}));

const api = vi.hoisted(() => ({
  installUpdateAndRestart: vi.fn(),
  openPortableUpdateDownload: vi.fn(),
  openExternal: vi.fn(),
  checkUpdates: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

vi.mock("@tauri-apps/api/app", () => ({ getVersion: async () => "4.0.20" }));

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: { available: true } }),
}));

vi.mock("@/contexts/UpdateContext", () => ({
  useUpdate: () => ({
    hasUpdate: updateState.hasUpdate,
    updateInfo: updateState.updateInfo,
    isChecking: false,
    checkUpdate: vi.fn().mockResolvedValue(true),
    resetDismiss: vi.fn(),
  }),
}));

vi.mock("@/lib/api", () => ({ settingsApi: api }));

vi.mock("@/extensions/prompt-rewriter/api", () => ({
  promptRewriterApi: { get: vi.fn() },
  promptRewriterKeys: { status: ["promptRewriter", "status"] },
}));

vi.mock("@/lib/toast", () => ({
  toast: { info: vi.fn(), success: vi.fn(), error: vi.fn() },
}));

vi.mock("@/components/WhatsNewDialog", () => ({ WhatsNewDialog: () => null }));

describe("AboutSection custom build updates", () => {
  beforeEach(() => {
    updateState.hasUpdate = true;
    api.installUpdateAndRestart.mockReset();
    api.openPortableUpdateDownload.mockReset();
    api.openExternal.mockReset();
  });

  it("keeps the update notice and opens an explanation instead of installing", async () => {
    render(<AboutSection isPortable />);

    const button = await screen.findByRole("button", {
      name: /promptRewriter.updateSource/,
    });
    fireEvent.click(button);

    expect(
      await screen.findByText("promptRewriter.updateBlockedTitle"),
    ).toBeInTheDocument();
    expect(api.installUpdateAndRestart).not.toHaveBeenCalled();
    expect(api.openPortableUpdateDownload).not.toHaveBeenCalled();
  });

  it("opens release notes from the dialog and closes it", async () => {
    render(<AboutSection isPortable />);

    fireEvent.click(
      await screen.findByRole("button", {
        name: /promptRewriter.updateSource/,
      }),
    );

    fireEvent.click(
      await screen.findByRole("button", {
        name: /promptRewriter.openReleaseNotes/,
      }),
    );

    expect(api.openExternal).toHaveBeenCalledWith(
      "https://github.com/farion1231/cc-switch/releases/tag/v4.0.21",
    );
    expect(
      screen.queryByText("promptRewriter.updateBlockedTitle"),
    ).not.toBeInTheDocument();
  });
});
