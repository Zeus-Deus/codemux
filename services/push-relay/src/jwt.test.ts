// @vitest-environment node
import { describe, expect, it } from "vitest";
import { base64url, constantTimeEqual, importPrivateKey, sha256Hex, signJwt } from "./jwt";
import { decodeJwt, generateEcKeyPair, generateRsaKeyPair, toPkcs8Pem, verifyJwt } from "./test-helpers";

describe("jwt helpers", () => {
  it("base64url-encodes without padding or url-unsafe characters", () => {
    expect(base64url(new Uint8Array([0xfb, 0xff, 0xfe]))).toBe("-__-");
    expect(base64url("hi")).toBe("aGk");
  });

  it("hashes to hex and compares in constant time", async () => {
    expect(await sha256Hex("abc")).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
    expect(constantTimeEqual("abcd", "abcd")).toBe(true);
    expect(constantTimeEqual("abcd", "abce")).toBe(false);
    expect(constantTimeEqual("abcd", "abc")).toBe(false);
  });

  it("signs ES256 JWTs verifiable with the matching public key", async () => {
    const pair = await generateEcKeyPair();
    const key = await importPrivateKey(await toPkcs8Pem(pair.privateKey), "ES256");
    const jwt = await signJwt(key, { alg: "ES256", kid: "KID" }, { iss: "TEAM", iat: 1 });
    const decoded = decodeJwt(jwt);
    expect(decoded.header).toEqual({ alg: "ES256", kid: "KID" });
    expect(decoded.claims).toEqual({ iss: "TEAM", iat: 1 });
    expect(decoded.signature).toHaveLength(64);
    expect(await verifyJwt(jwt, pair.publicKey)).toBe(true);
    const other = await generateEcKeyPair();
    expect(await verifyJwt(jwt, other.publicKey)).toBe(false);
  });

  it("signs RS256 JWTs and accepts PEMs with escaped newlines", async () => {
    const pair = await generateRsaKeyPair();
    const escapedPem = (await toPkcs8Pem(pair.privateKey)).replace(/\n/g, "\\n");
    const key = await importPrivateKey(escapedPem, "RS256");
    const jwt = await signJwt(key, { alg: "RS256", typ: "JWT" }, { iss: "svc" });
    expect(await verifyJwt(jwt, pair.publicKey)).toBe(true);
  });
});
