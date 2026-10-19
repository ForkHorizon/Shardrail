export type {
  ClientFrame,
  ClientHello,
  CommandRequest,
  DaemonFrame,
  DurableEvent,
  ServerHello,
} from "../../contracts/src/generated.js";

/** Browser clients cannot open the daemon's local Unix socket directly. */
export function connect(): never {
  throw new Error(
    "No client transport is available yet; use a native Unix socket relay.",
  );
}
