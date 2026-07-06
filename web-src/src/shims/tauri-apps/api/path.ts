export async function homeDir(): Promise<string> {
  return "/";
}

export async function join(...parts: string[]): Promise<string> {
  return parts.filter(Boolean).join("/");
}
