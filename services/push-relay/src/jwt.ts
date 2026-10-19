const encoder = new TextEncoder();

export function base64url(input: ArrayBuffer | Uint8Array | string): string {
  const bytes =
    typeof input === "string"
      ? encoder.encode(input)
      : input instanceof Uint8Array
        ? input
        : new Uint8Array(input);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Decodes a PKCS8 PEM. Tolerates literal "\n" sequences from env vars / JSON. */
export function pemToDer(pem: string): ArrayBuffer {
  const b64 = pem
    .replace(/\\n/g, "\n")
    .replace(/-----(BEGIN|END) [A-Z ]+-----/g, "")
    .replace(/\s+/g, "");
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

export type JwtAlg = "ES256" | "RS256";

const ALGORITHMS = {
  ES256: {
    importParams: { name: "ECDSA", namedCurve: "P-256" } as EcKeyImportParams,
    signParams: { name: "ECDSA", hash: "SHA-256" } as EcdsaParams,
  },
  RS256: {
    importParams: { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" } as RsaHashedImportParams,
    signParams: { name: "RSASSA-PKCS1-v1_5" } as Algorithm,
  },
};

export function importPrivateKey(pem: string, alg: JwtAlg): Promise<CryptoKey> {
  return crypto.subtle.importKey("pkcs8", pemToDer(pem), ALGORITHMS[alg].importParams, false, [
    "sign",
  ]);
}

/** WebCrypto ECDSA already yields raw r||s (64 bytes), which is exactly JWS ES256. */
export async function signJwt(
  key: CryptoKey,
  header: { alg: JwtAlg } & Record<string, string>,
  claims: Record<string, unknown>,
): Promise<string> {
  const signingInput = `${base64url(JSON.stringify(header))}.${base64url(JSON.stringify(claims))}`;
  const signature = await crypto.subtle.sign(
    ALGORITHMS[header.alg].signParams,
    key,
    encoder.encode(signingInput),
  );
  return `${signingInput}.${base64url(signature)}`;
}

export function randomBase64url(byteLength: number): string {
  return base64url(crypto.getRandomValues(new Uint8Array(byteLength)));
}

export async function sha256Hex(value: string): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", encoder.encode(value)));
  return Array.from(digest, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function constantTimeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}
