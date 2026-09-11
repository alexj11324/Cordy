"use client"

import { Button } from "@orvilo/ui/components/ui/button"
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
  FieldSeparator,
} from "@orvilo/ui/components/ui/field"
import { Input } from "@orvilo/ui/components/ui/input"
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@orvilo/ui/components/ui/select"

const ROLE_ITEMS = [
  { value: "admin", label: "Admin" },
  { value: "member", label: "Member" },
  { value: "viewer", label: "Viewer" },
] as const

export function Pattern() {
  return (
    <div className="mx-auto w-full max-w-lg">
      <FieldGroup>
        <Field orientation="responsive">
          <FieldLabel htmlFor="resp-first">First Name</FieldLabel>
          <Input id="resp-first" placeholder="First name" />
        </Field>
        <Field orientation="responsive">
          <FieldLabel htmlFor="resp-last">Last Name</FieldLabel>
          <Input id="resp-last" placeholder="Last name" />
        </Field>
        <Field orientation="responsive">
          <FieldLabel htmlFor="resp-email">Email</FieldLabel>
          <Input id="resp-email" type="email" placeholder="you@example.com" />
        </Field>
        <Field orientation="responsive">
          <FieldLabel htmlFor="resp-role">Role</FieldLabel>
          <Select defaultValue="member" items={[...ROLE_ITEMS]}>
            <SelectTrigger id="resp-role">
              <SelectValue placeholder="Select role" />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                {ROLE_ITEMS.map((role) => (
                  <SelectItem key={role.value} value={role.value}>
                    {role.label}
                  </SelectItem>
                ))}
              </SelectGroup>
            </SelectContent>
          </Select>
        </Field>
        <FieldSeparator />
        <FieldDescription>
          The invited member will receive an email with a link to join.
        </FieldDescription>
        <div className="flex justify-end gap-2">
          <Button variant="outline">Cancel</Button>
          <Button>Send Invite</Button>
        </div>
      </FieldGroup>
    </div>
  )
}
