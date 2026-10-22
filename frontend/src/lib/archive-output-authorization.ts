import { isErrorDto, type CreateDestinationInspectionDto, type ErrorDto } from "./ipc";

export class DestinationInspectionError extends Error {
  readonly detail: ErrorDto | null;
  readonly cancelled: boolean;

  constructor(error?: unknown, cancelled = false) {
    super("destination-inspection-failed");
    this.detail = isErrorDto(error) ? error : null;
    this.cancelled = cancelled;
  }
}

export function destinationInspectionCancelled(error: unknown): boolean {
  return error instanceof DestinationInspectionError
    && (error.cancelled || error.detail?.key === "error.cancelled");
}

export type ArchiveOutputAuthorization = Readonly<{
  replaceExisting: boolean;
  replacementGuard: string | null;
}>;
type AuthorizedOutputSpec = {
  replace_existing: boolean;
  replacement_guard: string | null;
};
type AuthorizationOptions = Readonly<{
  path: string;
  inspect: () => Promise<CreateDestinationInspectionDto>;
  isCurrent: () => boolean;
  confirm: () => Promise<typeof import("@tauri-apps/plugin-dialog")["confirm"]>;
  tr: (key: string) => string;
  message:
    | "gui.create.replace_existing"
    | "gui.output.replace_existing"
    | "gui.output.replace_existing_split"
    | "gui.create.replace_changed"
    | "gui.convert.replace_changed";
}>;

export async function authorizeArchiveOutput(
  options: AuthorizationOptions,
  previous?: AuthorizedOutputSpec,
): Promise<ArchiveOutputAuthorization | null> {
  if (!options.isCurrent()) return null;
  const inspection = await options.inspect();
  if (!options.isCurrent()) return null;
  if (inspection.conflict !== (inspection.guard !== null)) {
    throw new DestinationInspectionError();
  }
  const alreadyAuthorized = previous?.replace_existing && previous.replacement_guard === inspection.guard;
  if (inspection.conflict && !alreadyAuthorized) {
    const confirm = await options.confirm();
    if (!options.isCurrent()) return null;
    const buttons = options.message === "gui.output.replace_existing_split"
      ? "gui.output.replace_existing"
      : options.message;
    const replace = await confirm(
      options.tr(`${options.message}.body`).replace("{path}", options.path),
      {
        title: options.tr(`${options.message}.title`),
        kind: "warning",
        okLabel: options.tr(`${buttons}.action`),
        cancelLabel: options.tr(`${buttons}.cancel`),
      },
    );
    if (!options.isCurrent() || !replace) return null;
  }
  return {
    replaceExisting: inspection.conflict,
    replacementGuard: inspection.guard,
  };
}

export async function refreshArchiveOutputAuthorization<T extends AuthorizedOutputSpec>(
  spec: T,
  options: AuthorizationOptions,
): Promise<T | null> {
  const authorization = await authorizeArchiveOutput(options, spec);
  if (!authorization) return null;
  if (spec.replace_existing === authorization.replaceExisting
    && spec.replacement_guard === authorization.replacementGuard) return spec;
  return {
    ...spec,
    replace_existing: authorization.replaceExisting,
    replacement_guard: authorization.replacementGuard,
  };
}
