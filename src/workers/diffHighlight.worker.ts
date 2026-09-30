import { tokenizeDiff } from '../lib/diffTokenizer'
import type { SyntaxRequest, SyntaxResponse } from '../lib/diffSyntax'

const worker = self as unknown as {
  onmessage: ((event: MessageEvent<SyntaxRequest>) => void) | null
  postMessage(message: SyntaxResponse): void
}

worker.onmessage = async ({ data }) => {
  let tokens: SyntaxResponse['tokens'] = null
  try {
    tokens = await tokenizeDiff(data.path, data.hunks)
  } catch {
    // Highlighting is optional; the view already displays the original text.
  }
  worker.postMessage({ id: data.id, tokens })
}
