"use client"

import { type ReactNode } from "react"

import { AnthropicBlack } from "@orvilo/ui/components/ui/svgs/anthropicBlack"
import { AnthropicWhite } from "@orvilo/ui/components/ui/svgs/anthropicWhite"
import { Convex } from "@orvilo/ui/components/ui/svgs/convex"
import { Fastapi } from "@orvilo/ui/components/ui/svgs/fastapi"
import { Mintlify } from "@orvilo/ui/components/ui/svgs/mintlify"
import { N8n } from "@orvilo/ui/components/ui/svgs/n8n"
import { Neon } from "@orvilo/ui/components/ui/svgs/neon"
import { Openai } from "@orvilo/ui/components/ui/svgs/openai"
import { OpenaiDark } from "@orvilo/ui/components/ui/svgs/openaiDark"
import { Paper } from "@orvilo/ui/components/ui/svgs/paper"
import { Prisma } from "@orvilo/ui/components/ui/svgs/prisma"
import { PrismaDark } from "@orvilo/ui/components/ui/svgs/prismaDark"
import { RemixDark } from "@orvilo/ui/components/ui/svgs/remixDark"
import { RemixLight } from "@orvilo/ui/components/ui/svgs/remixLight"
import { Stripe } from "@orvilo/ui/components/ui/svgs/stripe"
import { Supabase } from "@orvilo/ui/components/ui/svgs/supabase"

// ── Types ──

export type Status = "Active" | "Inactive" | "Pending" | "Unstable" | "Blocked"
export type Availability = "online" | "away" | "busy" | "offline"
export type CompanyTier = "Enterprise" | "Pro" | "Starter"

/** Extra ledger context shown in the balance column (refunds, disputes, trends). */
export interface BalanceMeta {
  /** Card or bank reversal / chargeback already netted into balance */
  reversal?: {
    amount: number
    reason: string
    settledOn: string
  }
  /** vs prior statement period */
  trendPct?: number
  /** Not yet available for payout (Stripe-style) */
  pendingSettlement?: number
}

export interface IEmployee {
  id: string
  name: string
  availability: Availability
  avatar: string
  status: Status
  flag: string
  email: string
  company: string
  companyLogo: ReactNode
  role: string
  joined: string
  location: string
  balance: number
  customerId?: string
  department?: string
  tenureLabel?: string
  companyTier?: CompanyTier
  timezone?: string
  balanceMeta?: BalanceMeta
  lastActiveLabel?: string
  /** Replaces letter initials inside the customer Avatar when set. */
  avatarMedia?: ReactNode
  /** Replaces the location flag when set. */
  locationMedia?: ReactNode
  /** Replaces the currency-formatted balance when set. */
  balanceLabel?: string
  /** Numeric source used for chronological sorting while `joined` stays formatted. */
  joinedTimestamp?: number
  /** Product row permissions and lifecycle state used by the action cell. */
  canManage?: boolean
  isArchived?: boolean
  isSystemAgent?: boolean
}

// ── Logos ──

const ANTHROPIC_LOGO = (
  <>
    <span aria-hidden className="dark:hidden">
      <AnthropicBlack className="size-4" />
    </span>
    <span aria-hidden className="hidden dark:block">
      <AnthropicWhite className="size-4" />
    </span>
  </>
)

const OPENAI_LOGO = (
  <>
    <span aria-hidden className="dark:hidden">
      <Openai className="size-4" />
    </span>
    <span aria-hidden className="hidden dark:block">
      <OpenaiDark className="size-4" />
    </span>
  </>
)

const REMIX_LOGO = (
  <>
    <span aria-hidden className="dark:hidden">
      <RemixLight className="size-4" />
    </span>
    <span aria-hidden className="hidden dark:block">
      <RemixDark className="size-4" />
    </span>
  </>
)

const PRISMA_LOGO = (
  <>
    <span aria-hidden className="dark:hidden">
      <Prisma className="size-4" />
    </span>
    <span aria-hidden className="hidden dark:block">
      <PrismaDark className="size-4" />
    </span>
  </>
)

// ── Data ──

export const EMPLOYEES: IEmployee[] = [
  {
    id: "1",
    name: "Alex Johnson",
    availability: "online",
    avatar:
      "https://images.unsplash.com/photo-1535713875002-d1d0cf377fde?w=96&h=96&dpr=2&q=80",
    status: "Active",
    flag: "us",
    email: "alex@anthropic.com",
    company: "Anthropic",
    companyLogo: ANTHROPIC_LOGO,
    role: "CEO",
    department: "Executive",
    joined: "Jan, 2024",
    tenureLabel: "14 mo on Enterprise",
    location: "United States",
    timezone: "PST (UTC−8)",
    balance: 5143.03,
    customerId: "cus_7K2m9xQp",
    companyTier: "Enterprise",
    lastActiveLabel: "Active now",
    balanceMeta: {
      trendPct: 5.2,
      pendingSettlement: 890.0,
    },
  },
  {
    id: "2",
    name: "Sarah Chen",
    availability: "away",
    avatar:
      "https://images.unsplash.com/photo-1519699047748-de8e457a634e?w=96&h=96&dpr=2&q=80",
    status: "Inactive",
    flag: "gb",
    email: "sarah@openai.com",
    company: "OpenAI",
    companyLogo: OPENAI_LOGO,
    role: "CTO",
    department: "Engineering · L7",
    joined: "Mar, 2023",
    tenureLabel: "24 mo · renewal Mar 28",
    location: "United Kingdom",
    timezone: "GMT (UTC+0)",
    balance: 4321.87,
    customerId: "cus_3Nn8wLr1",
    companyTier: "Enterprise",
    lastActiveLabel: "Away · 42m ago",
    balanceMeta: {
      trendPct: -2.1,
      reversal: {
        amount: 420.0,
        reason: "Issuer dispute · provisional credit reversed",
        settledOn: "Mar 12, 2025",
      },
    },
  },
  {
    id: "3",
    name: "Michael Rodriguez",
    availability: "busy",
    avatar:
      "https://images.unsplash.com/photo-1584308972272-9e4e7685e80f?w=96&h=96&dpr=2&q=80",
    status: "Blocked",
    flag: "ca",
    email: "michael@remix.run",
    company: "Remix",
    companyLogo: REMIX_LOGO,
    role: "Designer",
    department: "Brand · IC4",
    joined: "Jun, 2022",
    tenureLabel: "33 mo · billing hold",
    location: "Canada",
    timezone: "EST (UTC−5)",
    balance: 7654.98,
    customerId: "cus_blocked_9x",
    companyTier: "Pro",
    lastActiveLabel: "Session locked",
    balanceMeta: {
      trendPct: -18.4,
      reversal: {
        amount: 2100.0,
        reason: "Chargeback cluster · risk review",
        settledOn: "Feb 02, 2025",
      },
    },
  },
  {
    id: "4",
    name: "Emma Wilson",
    availability: "offline",
    avatar:
      "https://images.unsplash.com/photo-1485893086445-ed75865251e0?w=96&h=96&dpr=2&q=80",
    status: "Inactive",
    flag: "au",
    email: "emma@convex.dev",
    company: "Convex",
    companyLogo: <Convex className="size-4" aria-hidden="true" />,
    role: "Developer",
    department: "Platform · IC3",
    joined: "Sep, 2024",
    tenureLabel: "6 mo on Pro trial",
    location: "Australia",
    timezone: "AEDT (UTC+11)",
    balance: 3456.45,
    customerId: "cus_0pL4vBn2",
    companyTier: "Pro",
    lastActiveLabel: "Offline · 2d",
    balanceMeta: {
      trendPct: 0.8,
      pendingSettlement: 120.5,
    },
  },
  {
    id: "5",
    name: "David Kim",
    availability: "online",
    avatar:
      "https://images.unsplash.com/photo-1607990281513-2c110a25bd8c?w=96&h=96&dpr=2&q=80",
    status: "Active",
    flag: "de",
    email: "david@stripe.com",
    company: "Stripe",
    companyLogo: <Stripe className="size-4" aria-hidden="true" />,
    role: "Lawyer",
    department: "Legal · Counsel",
    joined: "Nov, 2023",
    tenureLabel: "16 mo",
    location: "Germany",
    timezone: "CET (UTC+1)",
    balance: 9876.54,
    customerId: "cus_legal_88",
    companyTier: "Enterprise",
    lastActiveLabel: "Active now",
    balanceMeta: {
      trendPct: 12.7,
    },
  },
  {
    id: "6",
    name: "Aron Thompson",
    availability: "away",
    avatar:
      "https://images.unsplash.com/photo-1527980965255-d3b416303d12?w=96&h=96&dpr=2&q=80",
    status: "Pending",
    flag: "my",
    email: "aron@supabase.com",
    company: "Supabase",
    companyLogo: <Supabase className="size-4" aria-hidden="true" />,
    role: "Director",
    department: "GTM",
    joined: "Feb, 2022",
    tenureLabel: "37 mo · MRR review",
    location: "Malaysia",
    timezone: "MYT (UTC+8)",
    balance: 6214.22,
    customerId: "cus_pending_4j",
    companyTier: "Pro",
    lastActiveLabel: "Away · 3h",
    balanceMeta: {
      pendingSettlement: 2400.0,
      trendPct: 1.2,
    },
  },
  {
    id: "7",
    name: "James Brown",
    availability: "busy",
    avatar:
      "https://images.unsplash.com/photo-1543299750-19d1d6297053?w=96&h=96&dpr=2&q=80",
    status: "Inactive",
    flag: "es",
    email: "james@mintlify.com",
    company: "Mintlify",
    companyLogo: <Mintlify className="size-4" aria-hidden="true" />,
    role: "Product Manager",
    department: "Product",
    joined: "Aug, 2024",
    tenureLabel: "7 mo",
    location: "Spain",
    timezone: "CET (UTC+1)",
    balance: 5321.77,
    customerId: "cus_9mK2nX1",
    companyTier: "Starter",
    lastActiveLabel: "In meeting",
    balanceMeta: {
      trendPct: -0.4,
      reversal: {
        amount: 79.99,
        reason: "Refund · annual plan proration",
        settledOn: "Mar 01, 2025",
      },
    },
  },
  {
    id: "8",
    name: "Maria Garcia",
    availability: "offline",
    avatar:
      "https://images.unsplash.com/photo-1620075225255-8c2051b6c015?w=96&h=96&dpr=2&q=80",
    status: "Blocked",
    flag: "jp",
    email: "maria@paper.design",
    company: "Paper",
    companyLogo: <Paper className="size-4" aria-hidden="true" />,
    role: "Marketing Lead",
    department: "Marketing",
    joined: "Dec, 2023",
    tenureLabel: "15 mo · suspended",
    location: "Japan",
    timezone: "JST (UTC+9)",
    balance: 8452.39,
    customerId: "cus_risk_2w",
    companyTier: "Pro",
    lastActiveLabel: "Offline · 5d",
    balanceMeta: {
      trendPct: -6.0,
    },
  },
  {
    id: "9",
    name: "Nick Johnson",
    availability: "online",
    avatar:
      "https://images.unsplash.com/photo-1485206412256-701ccc5b93ca?w=96&h=96&dpr=2&q=80",
    status: "Pending",
    flag: "fr",
    email: "nick@n8n.io",
    company: "N8n",
    companyLogo: <N8n className="size-4" aria-hidden="true" />,
    role: "Data Scientist",
    department: "Data · ML platform",
    joined: "Apr, 2022",
    tenureLabel: "35 mo · KYC pending",
    location: "France",
    timezone: "CET (UTC+1)",
    balance: 7345.1,
    customerId: "cus_kyc_7f",
    companyTier: "Enterprise",
    lastActiveLabel: "Active now",
    balanceMeta: {
      trendPct: 3.9,
      pendingSettlement: 512.0,
    },
  },
  {
    id: "10",
    name: "Liam Thompson",
    availability: "away",
    avatar:
      "https://images.unsplash.com/photo-1542595913-85d69b0edbaf?w=96&h=96&dpr=2&q=80",
    status: "Inactive",
    flag: "it",
    email: "liam@fastapi.dev",
    company: "FastAPI",
    companyLogo: <Fastapi className="size-4" aria-hidden="true" />,
    role: "Engineer",
    department: "Backend · Platform",
    joined: "Jul, 2024",
    tenureLabel: "8 mo",
    location: "Italy",
    timezone: "CET (UTC+1)",
    balance: 5214.88,
    customerId: "cus_2vB8nL0",
    companyTier: "Starter",
    lastActiveLabel: "Away · 1h",
    balanceMeta: {
      trendPct: 2.2,
    },
  },
  {
    id: "11",
    name: "Sofia Martinez",
    availability: "busy",
    avatar:
      "https://images.unsplash.com/photo-1534528741775-53994a69daeb?w=96&h=96&dpr=2&q=80",
    status: "Active",
    flag: "br",
    email: "sofia@neon.tech",
    company: "Neon",
    companyLogo: <Neon className="size-4" aria-hidden="true" />,
    role: "Software Engineer",
    department: "Infra · Databases",
    joined: "May, 2023",
    tenureLabel: "22 mo",
    location: "Brazil",
    timezone: "BRT (UTC−3)",
    balance: 9421.5,
    customerId: "cus_neon_5a",
    companyTier: "Enterprise",
    lastActiveLabel: "In focus session",
    balanceMeta: {
      trendPct: 8.1,
      pendingSettlement: 150.0,
    },
  },
  {
    id: "12",
    name: "Oliver Park",
    availability: "online",
    avatar:
      "https://images.unsplash.com/photo-1506794778202-cad84cf45f1d?w=96&h=96&dpr=2&q=80",
    status: "Active",
    flag: "in",
    email: "oliver@prisma.io",
    company: "Prisma",
    companyLogo: PRISMA_LOGO,
    role: "Sales Manager",
    department: "Sales · Mid-market",
    joined: "Oct, 2024",
    tenureLabel: "5 mo · ramp",
    location: "India",
    timezone: "IST (UTC+5:30)",
    balance: 4521.67,
    customerId: "cus_6xR3mK9",
    companyTier: "Pro",
    lastActiveLabel: "Active now",
    balanceMeta: {
      trendPct: 4.4,
      reversal: {
        amount: 199.0,
        reason: "ACH return · insufficient funds",
        settledOn: "Mar 08, 2025",
      },
    },
  },
]

/** Stable order for filters and analytics. */
export const STATUS_ORDER: Status[] = [
  "Active",
  "Inactive",
  "Pending",
  "Unstable",
  "Blocked",
]

/** ~20% of rows (deterministic pseudo-random) use an Info tooltip for balance trend instead of inline arrows. */
export function rowShowsBalanceHint(id: string): boolean {
  const n = parseInt(id, 10)
  if (Number.isNaN(n)) return false
  return (n * 11 + 7) % 5 === 0
}

/** Tooltip copy when this row is in the hint cohort and has a trend. */
export function getBalanceHintText(
  row: Pick<IEmployee, "id" | "balanceMeta">
): string | null {
  if (!rowShowsBalanceHint(row.id)) return null
  const t = row.balanceMeta?.trendPct
  if (t === undefined) return null
  const up = t >= 0
  return `${up ? "Up" : "Down"} ${Math.abs(t).toFixed(1)}% vs prior period. Rolling 30-day comparison to the previous statement period.`
}
