/**
 * The browser half of passkeys: the server sends WebAuthn options with binary values as
 * base64url strings; these helpers turn them into what `navigator.credentials` takes and
 * turn its answers back into strings for the server. No credential material is kept.
 */

export function passkeysSupported(): boolean {
  return typeof window !== "undefined" && typeof window.PublicKeyCredential === "function" && Boolean(navigator.credentials);
}

export function toBase64Url(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function fromBase64Url(text: string): ArrayBuffer {
  const base64 = text.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(text.length / 4) * 4, "=");
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

type Descriptor = { type: "public-key"; id: string };

interface CreationOptionsJson {
  rp: PublicKeyCredentialRpEntity;
  user: { id: string; name: string; displayName: string };
  challenge: string;
  pubKeyCredParams: PublicKeyCredentialParameters[];
  timeout?: number;
  excludeCredentials?: Descriptor[];
  authenticatorSelection?: AuthenticatorSelectionCriteria;
  attestation?: AttestationConveyancePreference;
}

interface RequestOptionsJson {
  rpId: string;
  challenge: string;
  timeout?: number;
  allowCredentials?: Descriptor[];
  userVerification?: UserVerificationRequirement;
}

const descriptors = (list: Descriptor[] = []): PublicKeyCredentialDescriptor[] =>
  list.map((d) => ({ type: d.type, id: fromBase64Url(d.id) }));

/** What the person did instead of finishing: cancelled, timed out, or no passkey here. */
export class PasskeyCancelled extends Error {}

function rethrow(error: unknown): never {
  // NotAllowedError covers cancelling the prompt and letting it time out.
  if (error instanceof DOMException && (error.name === "NotAllowedError" || error.name === "AbortError")) throw new PasskeyCancelled(error.message);
  if (error instanceof DOMException && error.name === "InvalidStateError") throw new Error("This device's passkey has already been added.");
  throw error;
}

/** `navigator.credentials.create()`; returns the fields the server's register endpoint takes. */
export async function createPasskey(options: CreationOptionsJson) {
  let credential: Credential | null;
  try {
    credential = await navigator.credentials.create({
      publicKey: {
        ...options,
        challenge: fromBase64Url(options.challenge),
        user: { ...options.user, id: fromBase64Url(options.user.id) },
        excludeCredentials: descriptors(options.excludeCredentials),
      },
    });
  } catch (error) {
    rethrow(error);
  }
  if (!credential) throw new PasskeyCancelled("No passkey was created.");
  const response = (credential as PublicKeyCredential).response as AuthenticatorAttestationResponse;
  return { client_data_json: toBase64Url(response.clientDataJSON), attestation_object: toBase64Url(response.attestationObject) };
}

/** `navigator.credentials.get()`; returns the fields the server's sign-in endpoint takes. */
export async function getPasskey(options: RequestOptionsJson) {
  let credential: Credential | null;
  try {
    credential = await navigator.credentials.get({
      publicKey: { ...options, challenge: fromBase64Url(options.challenge), allowCredentials: descriptors(options.allowCredentials) },
    });
  } catch (error) {
    rethrow(error);
  }
  if (!credential) throw new PasskeyCancelled("No passkey was chosen.");
  const pk = credential as PublicKeyCredential;
  const response = pk.response as AuthenticatorAssertionResponse;
  if (!response.userHandle) throw new Error("This passkey can't be used to sign in here. Sign in with your password.");
  return {
    credential_id: toBase64Url(pk.rawId),
    client_data_json: toBase64Url(response.clientDataJSON),
    authenticator_data: toBase64Url(response.authenticatorData),
    signature: toBase64Url(response.signature),
    user_handle: toBase64Url(response.userHandle),
  };
}

export type { CreationOptionsJson, RequestOptionsJson };
