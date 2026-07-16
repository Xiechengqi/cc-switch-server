import { invokeCommand } from "@/lib/runtime";

export interface EmailAuthStatus {
  authenticated: boolean;
  email?: string | null;
  expiresAt?: number | null;
  routerDomain?: string | null;
}

export interface EmailSessionMeResponse {
  authenticated: boolean;
  user?: { id: string; email: string } | null;
  expiresAt?: string | null;
  installationOwnerEmail?: string | null;
}

async function changeOwnerEmail(params: {
  routerDomain?: string;
  currentEmail: string;
  newEmail: string;
}): Promise<EmailAuthStatus> {
  return invokeCommand("email_auth_change_owner_email", params);
}

async function getStatus(): Promise<EmailAuthStatus> {
  return invokeCommand("email_auth_get_status");
}

async function sessionMe(): Promise<EmailSessionMeResponse> {
  return invokeCommand("email_auth_session_me");
}

export const emailAuthApi = {
  changeOwnerEmail,
  getStatus,
  sessionMe,
};
