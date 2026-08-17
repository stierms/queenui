import {
  forwardRef,
  type ButtonHTMLAttributes,
  type ComponentPropsWithoutRef,
  type ReactNode,
} from "react";
import { ChevronDown } from "lucide-react";
import { cva, type VariantProps } from "class-variance-authority";
import {
  Dialog,
  DropdownMenu,
  Popover,
  Switch as SwitchPrimitive,
  Tabs,
  Tooltip,
} from "radix-ui";
import { cn } from "./cn";

/*
 * Palette note: every colour here is an `@theme` token, per
 * docs/frontend-architecture.md — no hex literals. The hover/active tints
 * (`accent-bright`, `accent-deep`, `app-line-5`, `app-text-soft`,
 * `app-muted-warm`) used to be spelled as raw hex in this file while the CSS
 * layer spelled the same values a second time; they are named once in the
 * `@theme` block now and both layers reference that name.
 */
const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-2 rounded-lg border text-body font-semibold transition-[color,background-color,border-color,transform,opacity] duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/60 focus-visible:ring-offset-2 focus-visible:ring-offset-app-bg disabled:pointer-events-none disabled:opacity-45",
  {
    variants: {
      variant: {
        primary:
          "h-9 border-transparent bg-accent px-4 text-app-panel shadow-[0_1px_2px_rgba(0,0,0,.3)] hover:bg-accent-bright active:translate-y-px active:bg-accent-deep",
        secondary:
          "h-9 border-app-line-3 bg-app-panel-high px-4 text-app-text-soft hover:border-app-line-5",
        ghost:
          "h-9 border-transparent bg-transparent px-3 text-app-muted-warm hover:bg-app-panel-high hover:text-accent",
        danger:
          "h-9 border-claret/40 bg-transparent px-4 text-claret hover:border-claret/60 hover:bg-claret/12",
        icon: "size-9 border-app-line-3 bg-app-panel-high p-0 text-app-muted-warm hover:border-app-line-5 hover:text-accent",
      },
    },
    defaultVariants: { variant: "secondary" },
  },
);

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> &
  VariantProps<typeof buttonVariants>;

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  function Button({ className, variant, type = "button", ...props }, ref) {
    return (
      <button
        ref={ref}
        type={type}
        className={cn(buttonVariants({ variant }), className)}
        {...props}
      />
    );
  },
);

type TooltipButtonProps = ButtonProps & { tooltip: ReactNode };

export const TooltipButton = forwardRef<HTMLButtonElement, TooltipButtonProps>(
  function TooltipButton({ tooltip, ...props }, ref) {
    return (
      <Tooltip.Provider delayDuration={350} skipDelayDuration={100}>
        <Tooltip.Root>
          <Tooltip.Trigger asChild>
            <Button ref={ref} {...props} />
          </Tooltip.Trigger>
          <Tooltip.Portal>
            <Tooltip.Content
              sideOffset={7}
              collisionPadding={10}
              className="z-40 rounded-md border border-app-line-4 bg-app-panel-raised px-2.5 py-1.5 text-caption text-app-bone-soft shadow-xl transition-opacity select-none data-[state=closed]:opacity-0 data-[state=delayed-open]:opacity-100"
            >
              {tooltip}
              <Tooltip.Arrow className="fill-app-line-4" />
            </Tooltip.Content>
          </Tooltip.Portal>
        </Tooltip.Root>
      </Tooltip.Provider>
    );
  },
);

type SwitchProps = ComponentPropsWithoutRef<typeof SwitchPrimitive.Root>;

/** Ebony track that fills moss when on, with a bone thumb. */
export const Switch = forwardRef<HTMLButtonElement, SwitchProps>(
  function Switch({ className, ...props }, ref) {
    return (
      <SwitchPrimitive.Root
        ref={ref}
        className={cn(
          "inline-flex h-[24px] w-[42px] shrink-0 cursor-pointer items-center rounded-full border border-app-line-3 bg-app-bg p-[2px] transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/60 focus-visible:ring-offset-2 focus-visible:ring-offset-app-bg disabled:cursor-default disabled:opacity-45 data-[state=checked]:border-[var(--moss)] data-[state=checked]:bg-[var(--moss)]",
          className,
        )}
        {...props}
      >
        <SwitchPrimitive.Thumb className="block size-[18px] rounded-full bg-accent shadow-[0_1px_2px_rgba(0,0,0,.4)] transition-transform duration-150 data-[state=checked]:translate-x-[18px]" />
      </SwitchPrimitive.Root>
    );
  },
);

/**
 * A destructive confirmation.
 *
 * Lived inside LogsPage, which meant every other destructive action in the app
 * — removing an engine profile, clearing an opening book, disconnecting a
 * Lichess account — either re-implemented it or skipped confirmation entirely.
 * Radix owns focus trap, Escape and focus restore; the cancel action is what
 * Escape and an overlay click resolve to.
 */
export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  pending,
  onCancel,
  onConfirm,
}: {
  open: boolean;
  title: string;
  description: ReactNode;
  confirmLabel: string;
  pending: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) onCancel();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-20 bg-black/70 backdrop-blur-[7px]" />
        <Dialog.Content className="logs-confirm fixed top-1/2 left-1/2 z-[21] -translate-x-1/2 -translate-y-1/2 focus:outline-none">
          <Dialog.Title>{title}</Dialog.Title>
          {/* Radix renders this as a <p>, so keep the content inline. */}
          <Dialog.Description>{description}</Dialog.Description>
          <div className="logs-confirm-actions">
            <Dialog.Close asChild>
              {/* The safe action carries focus, as in the close guard. */}
              <Button variant="secondary" autoFocus>
                Cancel
              </Button>
            </Dialog.Close>
            <Button variant="danger" disabled={pending} onClick={onConfirm}>
              {confirmLabel}
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** Labelled `<select>` with the app's chevron affordance. */
export function SelectField({
  label,
  value,
  onChange,
  disabled,
  children,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="select-wrap">
      <select
        aria-label={label}
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
      >
        {children}
      </select>
      <ChevronDown size={15} aria-hidden="true" />
    </div>
  );
}

export type RowMenuItem = {
  key: string;
  label: string;
  danger?: boolean;
  disabled?: boolean;
  /** Why the item is unavailable; shown beside a disabled label. */
  hint?: string;
  onSelect: () => void;
};

/**
 * The `•••` overflow menu on a table row.
 *
 * Radix `DropdownMenu` rather than a bare popover, so the items get real menu
 * semantics: arrow-key roving, type-ahead, Escape, and focus returned to the
 * trigger. The row keeps a single 30px control, which is all the fleet grid
 * has room for.
 */
export function RowMenu({
  label,
  items,
}: {
  label: string;
  items: RowMenuItem[];
}) {
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button className="more-button" aria-label={label}>
          •••
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          sideOffset={6}
          className="z-40 min-w-[200px] rounded-lg border border-app-line bg-app-panel-high p-1 shadow-xl"
        >
          {items.map((item) => (
            <DropdownMenu.Item
              key={item.key}
              disabled={item.disabled}
              // Deferred: the menu restores focus to its trigger as it
              // closes, which would steal focus back from a dialog opened
              // synchronously here.
              onSelect={() => window.setTimeout(item.onSelect, 0)}
              className={cn(
                "flex cursor-pointer items-center justify-between gap-3 rounded-md px-2.5 py-2 text-small outline-none select-none",
                "data-[highlighted]:bg-app-panel data-[disabled]:cursor-default data-[disabled]:opacity-45",
                item.danger ? "text-claret" : "text-app-text",
              )}
            >
              {item.label}
              {item.disabled && item.hint && (
                <small className="text-app-muted">{item.hint}</small>
              )}
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

export { Dialog, DropdownMenu, Popover, Tabs, Tooltip };
