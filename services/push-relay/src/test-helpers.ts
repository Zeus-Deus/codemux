// Shared fixtures for the push-relay tests (not exported from the package).

export async function toPkcs8Pem(key: CryptoKey): Promise<string> {
  const der = await crypto.subtle.exportKey("pkcs8", key);
  const b64 = Buffer.from(der).toString("base64");
  return `-----BEGIN PRIVATE KEY-----\n${b64.match(/.{1,64}/g)!.join("\n")}\n-----END PRIVATE KEY-----\n`;
}

export function generateEcKeyPair(): Promise<CryptoKeyPair> {
  return crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
}

export function generateRsaKeyPair(): Promise<CryptoKeyPair> {
  return crypto.subtle.generateKey(
    {
      name: "RSASSA-PKCS1-v1_5",
      modulusLength: 2048,
      publicExponent: new Uint8Array([1, 0, 1]),
      hash: "SHA-256",
    },
    true,
    ["sign", "verify"],
  );
}

export function decodeJwt(jwt: string) {
  const [h, c, s] = jwt.split(".");
  const decode = (part: string) => Buffer.from(part, "base64url");
  return {
    header: JSON.parse(decode(h).toString()) as Record<string, unknown>,
    claims: JSON.parse(decode(c).toString()) as Record<string, unknown>,
    signature: new Uint8Array(decode(s)),
    signingInput: new TextEncoder().encode(`${h}.${c}`),
  };
}

export async function verifyJwt(jwt: string, publicKey: CryptoKey): Promise<boolean> {
  const { header, signature, signingInput } = decodeJwt(jwt);
  const params =
    header.alg === "ES256" ? { name: "ECDSA", hash: "SHA-256" } : { name: "RSASSA-PKCS1-v1_5" };
  return crypto.subtle.verify(params, publicKey, signature, signingInput);
}
