import contract from "../../../contracts/auth-broker/v1.json";
export const AUTH_CONTRACT_VERSION = contract.version;
export const AUTH_CONTRACT_HEADER = "x-patchbay-auth-contract-version";
export const DESKTOP_ATTEMPT_PATH = contract.broker.desktopAttemptPath;
export const DESKTOP_COMPLETE_PATH = contract.broker.desktopCompletePath;
export const GO_ATTEMPT_PATH = contract.goApi.desktopAttemptPath;
export const GO_COMPLETE_PATH = contract.goApi.desktopCompletePath;
export function authContractResponseHeaders(): HeadersInit { return { [AUTH_CONTRACT_HEADER]: String(AUTH_CONTRACT_VERSION), "cache-control": "no-store" }; }
export { contract as AUTH_CONTRACT };
