export type ToastKind = "info" | "success" | "error";

export interface Toast {
  id: number;
  text: string;
  kind: ToastKind;
}

const DURATION_MS = 3500;

export const toasts: Toast[] = $state([]);
let nextId = 1;

export function toast(text: string, kind: ToastKind = "info") {
  const id = nextId++;
  toasts.push({ id, text, kind });
  setTimeout(() => dismiss(id), DURATION_MS);
}

export function dismiss(id: number) {
  const index = toasts.findIndex((t) => t.id === id);
  if (index >= 0) toasts.splice(index, 1);
}
