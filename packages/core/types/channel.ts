export type ChannelActorType = "member" | "agent";

export interface Channel {
  id: string;
  workspace_id: string;
  name: string;
  slug: string;
  description: string;
  created_by: string;
  archived_at?: string | null;
  created_at: string;
  updated_at: string;
}

export interface ChannelQuotedMessage {
  id: string;
  author_type: ChannelActorType;
  author_id: string;
  author_name: string;
  content: string;
}

export interface ChannelMessage {
  id: string;
  workspace_id: string;
  channel_id: string;
  author_type: ChannelActorType;
  author_id: string;
  author_name: string;
  author_avatar_url?: string | null;
  author_status?: string | null;
  content: string;
  parent_id?: string | null;
  quoted_message_id?: string | null;
  quoted_message?: ChannelQuotedMessage | null;
  created_at: string;
  updated_at: string;
}

export interface CreateChannelRequest {
  name: string;
  description?: string;
}

export interface SendChannelMessageRequest {
  content: string;
  parent_id?: string | null;
  quoted_message_id?: string | null;
}
