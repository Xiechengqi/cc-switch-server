export async function openExternal(url: string): Promise<void> {
  window.open(url, "_blank", "noopener,noreferrer");
}

export async function pickDirectory(): Promise<string | null> {
  return null;
}

export async function openFileDialog(): Promise<string | null> {
  return null;
}

export async function saveFileDialog(): Promise<string | null> {
  return null;
}

export async function openConfigFolder(): Promise<void> {
  return;
}

export async function openAppConfigFolder(): Promise<void> {
  return;
}
