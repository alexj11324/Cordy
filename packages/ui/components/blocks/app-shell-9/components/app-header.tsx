import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@orvilo/ui/components/ui/breadcrumb"
import { SidebarTrigger } from "@orvilo/ui/components/ui/sidebar"
import { BulletSeparator } from "./bullet-separator"
import { HouseIcon } from "lucide-react"

export function AppHeader() {
  return (
    <header className="sticky top-0 z-10 flex h-(--header-height) shrink-0 items-center justify-between border-b px-4">
      <div className="flex min-w-0 items-center gap-2">
        <SidebarTrigger className="-ml-1 md:hidden" />
        <Breadcrumb>
          <BreadcrumbList>
            <BreadcrumbItem className="hidden items-center md:flex">
              <BreadcrumbLink href="#" className="flex items-center gap-1.5">
                <HouseIcon className="size-4" aria-hidden="true" />
                Home
              </BreadcrumbLink>
            </BreadcrumbItem>
            <BreadcrumbSeparator className="hidden items-center md:flex">
              <BulletSeparator />
            </BreadcrumbSeparator>
            <BreadcrumbItem>
              <BreadcrumbPage>Overview</BreadcrumbPage>
            </BreadcrumbItem>
          </BreadcrumbList>
        </Breadcrumb>
      </div>
    </header>
  )
}