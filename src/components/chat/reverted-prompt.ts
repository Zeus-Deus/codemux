import type {
  ChatViewItem,
  UserMessageImage,
  UserMessageItem,
} from "@/lib/agent-chat/types";
import { readChatImage } from "@/tauri/commands";

/** The prompt a turn revert removes, captured so it can go back into the
 *  composer for editing. */
export interface RevertedPrompt {
  text: string;
  images: File[];
  /** Attached images that could not be read back. */
  missingImages: number;
}

export function findRevertedUserMessage(
  messages: readonly ChatViewItem[],
  clientNonce: string | null,
): UserMessageItem | null {
  if (!clientNonce) return null;
  for (const item of messages) {
    if (item.kind === "user_message" && item.clientNonce === clientNonce) {
      return item;
    }
  }
  return null;
}

function dataUrlBytes(src: string): { bytes: Uint8Array; mime: string } | null {
  const match = /^data:([^;,]+)?(;base64)?,(.*)$/s.exec(src);
  if (!match || !match[2]) return null;
  const binary = atob(match[3]);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return { bytes, mime: match[1] ?? "image/png" };
}

const EXTENSION_BY_MIME: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/webp": "webp",
  "image/gif": "gif",
};

async function imageFile(image: UserMessageImage, index: number): Promise<File> {
  const inline = image.src.startsWith("data:") ? dataUrlBytes(image.src) : null;
  const { bytes, mime } = inline
    ? inline
    : await readChatImage(image.src).then((read) => ({
        bytes: read.bytes,
        mime: read.media_type,
      }));
  const type = image.mediaType ?? mime;
  const name = `reverted-image-${index + 1}.${EXTENSION_BY_MIME[type] ?? "png"}`;
  return new File([bytes], name, { type });
}

/**
 * Read the message's text and image bytes. Runs before the revert, because
 * the revert deletes the image files only the removed turns referenced.
 */
export async function captureRevertedPrompt(
  message: UserMessageItem,
): Promise<RevertedPrompt> {
  const images = message.images ?? [];
  const settled = await Promise.allSettled(images.map(imageFile));
  const files: File[] = [];
  for (const result of settled) {
    if (result.status === "fulfilled") files.push(result.value);
  }
  return {
    text: message.text,
    images: files,
    missingImages: images.length - files.length,
  };
}

/** Put the reverted prompt ahead of anything already typed. */
export function mergeRevertedDraft(prompt: string, current: string): string {
  if (!prompt) return current;
  return current.trim().length > 0 ? `${prompt}\n${current}` : prompt;
}
