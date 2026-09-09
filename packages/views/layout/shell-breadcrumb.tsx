"use client";

import { AppLink, useNavigation } from "../navigation";
import { paths, useCurrentWorkspace, useWorkspaceSlug } from "@orvilo/core/paths";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@orvilo/ui/components/ui/breadcrumb";
import { useTabPresentation } from "./tab-presentation";

/**
 * Workspace › current page trail for the shell header.
 *
 * Replaces the desktop tab strip: one containment chain instead of stacked
 * tabs plus an in-page crumb. Titles come from the same presentation helper
 * the tab bar used, so a renamed issue still updates in place.
 */
export function ShellBreadcrumb() {
  const { pathname, searchParams } = useNavigation();
  const workspace = useCurrentWorkspace();
  const slug = useWorkspaceSlug();
  const search = searchParams?.toString() ?? "";
  const url = search ? `${pathname}?${search}` : pathname;
  const { title } = useTabPresentation(url);

  if (!workspace || !slug) return null;

  return (
    <Breadcrumb className="min-w-0">
      <BreadcrumbList className="flex-nowrap">
        <BreadcrumbItem className="flex min-w-0">
          <BreadcrumbLink
            render={<AppLink href={paths.workspace(slug).issues()} />}
            className="flex min-w-0 items-center"
          >
            <span className="truncate">{workspace.name}</span>
          </BreadcrumbLink>
        </BreadcrumbItem>
        <BreadcrumbSeparator className="shrink-0" />
        <BreadcrumbItem className="min-w-0">
          <BreadcrumbPage className="truncate">{title}</BreadcrumbPage>
        </BreadcrumbItem>
      </BreadcrumbList>
    </Breadcrumb>
  );
}
