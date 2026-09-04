import { render, screen } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import type { InputHTMLAttributes, ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import enWorkProducts from "../locales/en/work-products.json";
import { WorkProductRelationsSection } from "./work-product-relations-section";

const product = {
  id: "wp-1",
  workspace_id: "ws-1",
  kind: "pull_request",
  provider: "github",
  external_identity: "acme/repo#42",
  external_url: null,
  provider_record_type: "pull_request",
  provider_record_id: "record-42",
  created_at: "2026-09-02T00:00:00Z",
  updated_at: "2026-09-02T00:01:00Z",
};

const relationState = {
  data: {
    pages: [
      {
        relations: [
          {
            id: "relation-1",
            workspace_id: "ws-1",
            work_product_id: "wp-1",
            issue_id: "issue-1",
            task_id: null,
            run_id: null,
            relation_key: "manual:relation-1",
            relation_source: "manual_explicit",
            attached_by_type: "user",
            attached_by_id: "user-1",
            attached_at: "2026-09-02T00:02:00Z",
            close_intent: true,
            detached_at: null,
            detached_by_type: null,
            detached_by_id: null,
            detached_task_id: null,
            detached_run_id: null,
          },
        ],
        page: 1,
        per_page: 64,
        has_more: false,
      },
    ],
  },
  isPending: false,
  hasNextPage: false,
  isFetchingNextPage: false,
  fetchNextPage: vi.fn(),
};

const productListState = {
  ...relationState,
  data: {
    pages: [
      {
        work_products: [product],
        next_page: null,
      },
    ],
  },
};

const issueProductsState = {
  ...relationState,
  data: {
    pages: [
      {
        work_products: [
          {
            ...product,
            relation: {
              id: "relation-1",
              issue_id: "issue-1",
              task_id: null,
              run_id: null,
              relation_source: "manual_explicit",
              attached_by_type: "user",
              attached_by_id: "user-1",
              attached_at: "2026-09-02T00:02:00Z",
              close_intent: true,
            },
          },
        ],
        page: 1,
        per_page: 64,
        has_more: false,
      },
    ],
  },
};

const { attachExisting, attachPullRequest } = vi.hoisted(() => ({
  attachExisting: vi.fn(),
  attachPullRequest: vi.fn(),
}));

vi.mock("@patchbay/core/work-products", () => ({
  useAttachExistingWorkProduct: () => ({ isPending: false, mutate: attachExisting }),
  useAttachIssuePullRequest: () => ({ isPending: false, mutate: attachPullRequest }),
  useDetachWorkProduct: () => ({ isPending: false, mutate: vi.fn() }),
  issueWorkProductsInfiniteOptions: (_wsId: string | null, issueId: string) => ({
    queryKey: ["work-products", "issue", issueId, "infinite"],
  }),
  workProductDetailOptions: (_wsId: string | null, id: string) => ({
    queryKey: ["work-product-detail", id],
  }),
  unassociatedWorkProductListInfiniteOptions: () => ({ queryKey: ["work-products", "ws-1", "unassociated"] }),
}));

vi.mock("@tanstack/react-query", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tanstack/react-query")>()),
  useInfiniteQuery: ({ queryKey }: { queryKey: readonly unknown[] }) =>
    queryKey[1] === "issue" ? issueProductsState : productListState,
  useQueries: ({ queries }: { queries: readonly unknown[] }) =>
    queries.map(() => ({ data: product })),
}));

vi.mock("@patchbay/core/hooks", () => ({
  useWorkspaceId: () => "ws-1",
}));

vi.mock("@patchbay/core/paths", () => ({
  useWorkspacePaths: () => ({
    workProductDetail: (id: string) => `/acme/work-products/${id}`,
  }),
}));

vi.mock("../navigation", () => ({
  AppLink: ({ children, href }: { children: ReactNode; href: string }) => (
    <a href={href}>{children}</a>
  ),
}));

vi.mock("@patchbay/ui/components/ui/badge", () => ({
  Badge: ({ children }: { children: ReactNode }) => <span>{children}</span>,
}));

vi.mock("@patchbay/ui/components/ui/button", () => ({
  Button: ({
    children,
    onClick,
    disabled,
  }: {
    children: ReactNode;
    onClick?: () => void;
    disabled?: boolean;
  }) => (
    <button type="button" onClick={onClick} disabled={disabled}>
      {children}
    </button>
  ),
}));

vi.mock("@patchbay/ui/components/ui/checkbox", () => ({
  Checkbox: ({ checked }: { checked?: boolean }) => (
    <input type="checkbox" checked={checked} readOnly />
  ),
}));

vi.mock("@patchbay/ui/components/ui/input", () => ({
  Input: (props: InputHTMLAttributes<HTMLInputElement>) => <input {...props} />,
}));

vi.mock("@patchbay/ui/components/ui/dialog", () => ({
  Dialog: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  DialogContent: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  DialogDescription: ({ children }: { children: ReactNode }) => <p>{children}</p>,
  DialogFooter: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  DialogHeader: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  DialogTitle: ({ children }: { children: ReactNode }) => <h2>{children}</h2>,
}));

vi.mock("@patchbay/ui/components/ui/label", () => ({
  Label: ({ children }: { children: ReactNode }) => <label>{children}</label>,
}));

vi.mock("@patchbay/ui/components/ui/select", () => ({
  Select: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  SelectContent: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  SelectItem: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  SelectTrigger: ({ children }: { children: ReactNode }) => <button type="button">{children}</button>,
  SelectValue: ({ placeholder }: { placeholder?: string }) => <span>{placeholder}</span>,
}));

function renderSection() {
  return render(
    <I18nProvider
      locale="en"
      resources={{ en: { "work-products": enWorkProducts } }}
    >
      <WorkProductRelationsSection issueId="issue-1" />
    </I18nProvider>,
  );
}

describe("WorkProductRelationsSection", () => {
  it("renders the active relation through the workspace product route", () => {
    renderSection();

    expect(
      screen.getByRole("link", { name: "acme/repo#42" }),
    ).toHaveAttribute("href", "/acme/work-products/wp-1");
    expect(screen.getByText("close")).toBeInTheDocument();
  });
});
