import { invoke } from "@tauri-apps/api/core";

export interface EmailAuthStatus {
  authenticated: boolean;
  email?: string | null;
  expiresAt?: number | null;
  routerDomain?: string | null;
}

export interface EmailCodeRequestResponse {
  ok: boolean;
  cooldownSecs: number;
  maskedDestination: string;
}

export interface EmailAuthUser {
  id: string;
  email: string;
}

export interface EmailSessionMeResponse {
  authenticated: boolean;
  user?: EmailAuthUser | null;
  expiresAt?: string | null;
  installationOwnerEmail?: string | null;
}

async function requestCode(params: {
  routerDomain: string;
  email: string;
}): Promise<EmailCodeRequestResponse> {
  return invoke("email_auth_request_code", params);
}

async function verifyCode(
  routerDomain: string,
  email: string,
  code: string,
): Promise<EmailAuthStatus> {
  return invoke("email_auth_verify_code", { routerDomain, email, code });
}

async function requestOwnerChangeCode(params: {
  routerDomain: string;
  currentEmail: string;
  newEmail: string;
}): Promise<EmailCodeRequestResponse> {
  return invoke("email_auth_request_owner_change_code", params);
}

async function changeOwnerEmail(params: {
  routerDomain: string;
  currentEmail: string;
  newEmail: string;
  code: string;
}): Promise<EmailAuthStatus> {
  return invoke("email_auth_change_owner_email", params);
}

async function getStatus(): Promise<EmailAuthStatus> {
  return invoke("email_auth_get_status");
}

async function sessionMe(): Promise<EmailSessionMeResponse> {
  return invoke("email_auth_session_me");
}

async function logout(): Promise<void> {
  return invoke("email_auth_logout");
}

export const emailAuthApi = {
  requestCode,
  verifyCode,
  requestOwnerChangeCode,
  changeOwnerEmail,
  getStatus,
  sessionMe,
  logout,
};
