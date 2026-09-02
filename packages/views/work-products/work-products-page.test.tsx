import { render, screen } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import enWorkProducts from "../locales/en/work-products.json";
import { WorkProductsPage } from "./work-products-page";

const { queryState } = vi.hoisted(() => ({
  queryState: {
    products: {
      data: {
        pages: [
          {
            products: [
              {
                id: "wp-1",
                workspace_id: "ws-1",
                kind: "pull_request",
                provider: "github",
                external_identity: "acme/repo#42",
                external_url: "https://github.com/acme/repo/pull/42",
                provider_record_type: "pull_request",
                provider_record_id: "record-42",
                created_at: "2026-09-02T00:00:00Z",
                updated_at: "2026-09-02T00:01:00Z",
              },
            ],
            page: 1,
            per_page: 64,
            has_more: false,
          },
        ],
      },
      isPending: false,
      isError: false,
      hasNextPage: false,
      isFetchingNextPage: false,
      refetch: vi.fn(),
      fetchNextPage: vi.fn(),
    },
    provenance: {
      data: {
        pages: [
          {
            provenance: [
              {
                task_id: "task-1",
                workspace_id: "ws-1",
                run_id: "run-1",
                repo_identity: "acme/repo",
                execution_workspace: "/workspaces/task-1",
                head_branch: "feature/work-product",
                head_sha: "0123456789abcdef0123456789abcdef01234567",
                head_state: "attached",
                started_at: "2026-09-02T00:00:00Z",
                finished_at: "2026-09-02T00:03:00Z",
                discovery_status: "matched",
                discovery_lease_id: "lease-1",
                discovery_match_count: 1,
                discovery_reason: "exact head SHA",
                discovery_work_product_id: "wp-1",
                discovery_at: "2026-09-02T00:04:00Z",
                updated_at: "2026-09-02T00:04:00Z",
              },
            ],
            page: 1,
            per_page: 64,
            has_more: false,
          },
        ],
      },
      isPending: false,
      isError: false,
      hasNextPage: false,
      isFetchingNextPage: false,
      refetch: vi.fn(),
      fetchNextPage: vi.fn(),
    },
  },
}));

vi.mock("@tanstack/react-query", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tanstack/react-query")>()),
  useInfiniteQuery: ({ queryKey }: { queryKey: readonly unknown[] }) =>
    queryKey.includes("provenance") ? queryState.provenance : queryState.products,
}));

vi.mock("@patchbay/core/hooks", () => ({
  useWorkspaceId: () => "ws-1",
}));

vi.mock("@patchbay/core/paths", () => ({
  useWorkspacePaths: () => ({
    workProductDetail: (id: string) => `/acme/work-products/${id}`,
    workProducts: () => "/acme/work-products",
  }),
}));

vi.mock("../navigation", () => ({
  AppLink: ({
    children,
    href,
  }: {
    children: ReactNode;
    href: string;
  }) => <a href={href}>{children}</a>,
}));

function renderPage() {
  return render(
    <I18nProvider
      locale="en"
      resources={{ en: { "work-products": enWorkProducts } }}
    >
      <WorkProductsPage />
    </I18nProvider>,
  );
}

describe("WorkProductsPage", () => {
  it("renders a workspace-scoped product link and the exact provenance head SHA", () => {
    renderPage();

    expect(screen.getByText("acme/repo#42")).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: /acme\/repo#42/i }),
    ).toHaveAttribute("href", "/acme/work-products/wp-1");
    expect(
      screen.getByText("0123456789abcdef0123456789abcdef01234567"),
    ).toBeInTheDocument();
  });
});
