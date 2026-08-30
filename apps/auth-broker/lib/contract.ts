import contract from "../../../contracts/auth-broker/v1.json";

export const AUTH_CONTRACT_VERSION = contract.version;
export const AUTH_CONTRACT_HEADER = "x-patchbay-auth-contract-version";
export const DESKTOP_ATTEMPT_PATH = contract.broker.desktopAttemptPath;
export const DESKTOP_COMPLETE_PATH = contract.broker.desktopCompletePath;
export const RUST_ATTEMPT_PATH = contract.rustApi.desktopAttemptPath;
export const RUST_COMPLETE_PATH = contract.rustApi.desktopCompletePath;
export const DESKTOP_CALLBACK_URL = contract.desktop.callbackUrl;

export function authContractResponseHeaders(): HeadersInit {
  return {
    [AUTH_CONTRACT_HEADER]: String(AUTH_CONTRACT_VERSION),
    "cache-control": "no-store",
  };
}

export { contract as AUTH_CONTRACT };
