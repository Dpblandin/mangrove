import { invoke, isTauri } from '@tauri-apps/api/core'

export type GitClient = <T>(
  command: string,
  args: Record<string, unknown>,
  signal?: AbortSignal,
) => Promise<T>
export const desktop = isTauri()

export const request: GitClient = async <T>(
  command: string,
  args: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<T> => {
  if (!desktop) throw new Error('Ouvrez Mangrove dans son application desktop.')
  if (signal?.aborted) throw new DOMException('Requête annulée', 'AbortError')
  return invoke<T>(command, args)
}

export async function chooseFolder(): Promise<string | null> {
  if (!desktop) return null
  const { open } = await import('@tauri-apps/plugin-dialog')
  return open({ directory: true, multiple: false, title: 'Ouvrir un dépôt Git' })
}
