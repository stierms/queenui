import { useId, type ReactNode } from "react";

export type PieceSetId =
  | "regal"
  | "staunton"
  | "ink"
  | "blueprint"
  | "deco"
  | "horn"
  | "nib"
  | "lamp"
  | "foundry"
  | "relay"
  | "chisel"
  | "matrix"
  | "optic"
  | "switchgear"
  | "kiln"
  | "compositor"
  | "aperture";
export type PieceKind = "p" | "n" | "b" | "r" | "q" | "k";
export type PieceColor = "w" | "b";

const pieceNames: Record<PieceKind, string> = {
  p: "pawn",
  n: "knight",
  b: "bishop",
  r: "rook",
  q: "queen",
  k: "king",
};

/** Sculpted tournament language: gradients, collar lines, carved details. */
function RegalGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <circle className="piece-body" cx="50" cy="27" r="13" />
          <path
            className="piece-body"
            d="M39 41c-7 7-7 17-1 24l-7 13h38l-7-13c6-7 6-17-1-24z"
          />
          <path className="piece-detail" d="M37 61h26M31 78h38M27 84h46" />
          <path className="piece-body piece-base" d="M27 78h46l5 9H22z" />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M25 17h14v10h8V17h7v10h8V17h14v25l-8 8 5 29H27l5-29-7-8z"
          />
          <path
            className="piece-detail"
            d="M27 40h47M32 50h36M29 74h42M25 80h50"
          />
          <path className="piece-body piece-base" d="M25 76h50l5 11H20z" />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M28 79c5-14 11-22 20-29l-14-5 3-7-5-3 2-6 9-2 2-5 8-4 2-8 4 6 5-7 3 10c7 4 11 10 13 18 3 11 1 20-9 26l4 16z"
          />
          <path
            className="piece-detail"
            d="M46 35c6 3 11 7 15 12M39 62c9-1 18 1 26 7"
          />
          <circle className="piece-eye" cx="55" cy="27" r="3.2" />
          <path className="piece-body piece-base" d="M27 76h49l6 11H21z" />
        </>
      );
    case "b":
      return (
        <>
          <circle className="piece-body" cx="50" cy="11" r="3.5" />
          <path
            className="piece-body"
            d="M50 14c12 10 18 19 18 29 0 8-4 14-10 19l10 17H32l10-17c-6-5-10-11-10-19 0-7 3-14 8-19l14 14 5-5-13-13c1-2 3-4 4-6z"
          />
          <path className="piece-detail" d="M38 59h24M32 77h36" />
          <path className="piece-body piece-base" d="M29 75h42l8 12H21z" />
        </>
      );
    case "q":
      return (
        <>
          <path
            className="piece-stem"
            d="M24 25V19M41 21V15M59 21V15M76 25V19"
          />
          <circle className="piece-jewel" cx="24" cy="17.5" r="4" />
          <circle className="piece-jewel" cx="41" cy="13.5" r="4" />
          <circle className="piece-jewel" cx="59" cy="13.5" r="4" />
          <circle className="piece-jewel" cx="76" cy="17.5" r="4" />
          <path
            className="piece-body"
            d="m24 25 12 17 5-21 9 20 9-20 5 21 12-17-8 39 4 15H28l4-15z"
          />
          <path className="piece-detail" d="M32 61h36M29 76h42" />
          <path className="piece-body piece-base" d="M27 75h46l7 12H20z" />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M46 10h8v10h10v8H54v14h-8V28H36v-8h10z"
          />
          <path
            className="piece-body"
            d="M50 35c14 0 22 9 19 20-1 5-5 9-10 12l10 12H31l10-12c-5-3-9-7-10-12-3-11 5-20 19-20z"
          />
          <path className="piece-detail" d="M34 52h32M39 65h22M31 77h38" />
          <path className="piece-body piece-base" d="M28 75h44l8 12H20z" />
        </>
      );
  }
}

/** Shared Staunton turning: molded pedestal under every piece. */
function StauntonPedestal({ narrow = false }: { narrow?: boolean }) {
  return narrow ? (
    <>
      <path
        className="piece-body"
        d="M34 72.5h32c3 2 4.5 4.2 4.5 6.5H29.5c0-2.3 1.5-4.5 4.5-6.5z"
      />
      <path
        className="piece-body piece-base"
        d="M30 79h40c4.5 2.8 6 5.2 6 8H24c0-2.8 1.5-5.2 6-8z"
      />
    </>
  ) : (
    <>
      <path
        className="piece-body"
        d="M33 72.5h34c3 2 4.5 4.2 4.5 6.5H28.5c0-2.3 1.5-4.5 4.5-6.5z"
      />
      <path
        className="piece-body piece-base"
        d="M28 79h44c5 2.8 6.5 5.2 6.5 8H21.5c0-2.8 1.5-5.2 6.5-8z"
      />
    </>
  );
}

/** Classical tournament language: ball-and-collar turning, molded bases. */
function StauntonGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <circle className="piece-body" cx="50" cy="24" r="10.5" />
          <path
            className="piece-body"
            d="M37 35.5h26a3.6 3.6 0 0 1 0 7.2H37a3.6 3.6 0 0 1 0-7.2z"
          />
          <path
            className="piece-body"
            d="M43.5 42.7c.4 11-2.4 21.6-8 29.8h29c-5.6-8.2-8.4-18.8-8-29.8z"
          />
          <StauntonPedestal narrow />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M31 13h10v5.5a2.4 3.2 0 0 0 4.8 0V13h8.4v5.5a2.4 3.2 0 0 0 4.8 0V13h10v13l-4.5 6c1.7 12.4 2.5 25.6 2.5 40.5h-34c0-14.9.8-28.1 2.5-40.5L31 26z"
          />
          <path className="piece-detail" d="M35.5 34.5h29M34 63.5h32" />
          <StauntonPedestal />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M31 76c1.5-9 4.5-17 9.5-23-4-1.2-7.8-3.6-11.5-7-2.8-2.6-5-5.4-6.5-8.5l5.5-.7-4.2-2.9c-1-2.5-.5-4.6 1.5-6.1 6-4.5 13-7.5 20-9l1.5-8 4.5 6.5 4-7.5 4.5 8c5.5 2.5 9.5 7 12 13 3.5 9 3 21.5-2.5 31.5l3 14.2z"
          />
          <path
            className="piece-detail"
            d="M38.5 42c4.5.3 8.6 2.3 12.2 5.8M58.5 25c4.5 6.5 6.3 14.5 5.8 23.5"
          />
          <circle className="piece-eye" cx="45" cy="26.5" r="2.9" />
          <StauntonPedestal />
        </>
      );
    case "b":
      return (
        <>
          <circle className="piece-body" cx="50" cy="10.5" r="4" />
          <path
            className="piece-body"
            d="M50 15.5c7 4.7 11.7 12.3 11.7 20.7 0 6.8-2.9 12.2-7.4 15.8H45.7c-4.5-3.6-7.4-9-7.4-15.8 0-6 2.4-11.7 6.3-16.2l10.5 11.4 3.5-4.5L48 15.9a29 29 0 0 1 2-.4z"
          />
          <path
            className="piece-body"
            d="M40 52h20a3.3 3.3 0 0 1 0 6.6H40a3.3 3.3 0 0 1 0-6.6z"
          />
          <path
            className="piece-body"
            d="M44.5 58.6c.5 5.4-1 10-4.5 13.9h20c-3.5-3.9-5-8.5-4.5-13.9z"
          />
          <StauntonPedestal narrow />
        </>
      );
    case "q":
      return (
        <>
          <circle className="piece-body" cx="50" cy="8.5" r="3.4" />
          <path
            className="piece-body"
            d="M28.5 32 26 14l6.5 10.5L38 12l6.5 10.5L50 13l5.5 9.5L62 12l5.5 12.5L74 14l-2.5 18z"
          />
          <path
            className="piece-body"
            d="M30.5 32h39c.8 4.6-1 8.3-4.4 10.6 1 .8 1.6 1.9 1.7 3.2-2.2 10.6-1.5 19.8 2.2 30.2H31c3.7-10.4 4.4-19.6 2.2-30.2.1-1.3.7-2.4 1.7-3.2C31.5 40.3 29.7 36.6 30.5 32z"
          />
          <path className="piece-detail" d="M35.5 46h29M37 52h26" />
          <StauntonPedestal />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M46.2 4.5c2.5 1.1 5.1 1.1 7.6 0l-1.3 6.1c.9.5 1.7 1.2 2.4 2l6.1-1.3c-1.1 2.5-1.1 5.1 0 7.6l-6.1-1.3c-.7.8-1.5 1.5-2.4 2l1.3 6.1c-2.5-1.1-5.1-1.1-7.6 0l1.3-6.1c-.9-.5-1.7-1.2-2.4-2L39 18.9c1.1-2.5 1.1-5.1 0-7.6l6.1 1.3c.7-.8 1.5-1.5 2.4-2z"
          />
          <path
            className="piece-body"
            d="M36 26.5h28a3.4 3.4 0 0 1 0 6.8H36a3.4 3.4 0 0 1 0-6.8z"
          />
          <path
            className="piece-body"
            d="M36.5 33.3c-3.6 4.6-5.1 9.6-4.1 14.7.9 4.4 3.4 8 6.9 10.6-1.9 6-3.6 11.9-5.3 17.4h32c-1.7-5.5-3.4-11.4-5.3-17.4 3.5-2.6 6-6.2 6.9-10.6 1-5.1-.5-10.1-4.1-14.7z"
          />
          <path className="piece-detail" d="M36.5 48.5h27M36 62.5h28" />
          <StauntonPedestal />
        </>
      );
  }
}

/** Ink: shared calligraphic foot — one smooth swell instead of turned moldings. */
function InkFoot({ narrow = false }: { narrow?: boolean }) {
  return narrow ? (
    <path
      className="piece-body piece-base"
      d="M27.5 87c1.2-6.2 4.1-9.5 8.7-10.5h27.6c4.6 1 7.5 4.3 8.7 10.5z"
    />
  ) : (
    <path
      className="piece-body piece-base"
      d="M25.5 87c1.2-6.6 4.3-10.2 9.3-11h30.4c5 .8 8.1 4.4 9.3 11z"
    />
  );
}

/** Flat printed-figurine language: solid glyph silhouettes, inline hairline. */
function InkGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <path
            className="piece-body"
            d="M50 14.5a10.5 10.5 0 0 1 5.4 19.6c7.5 5 11.8 13.7 12.9 26L69.7 76H30.3l1.4-15.9c1.1-12.3 5.4-21 12.9-26A10.5 10.5 0 0 1 50 14.5z"
          />
          <path className="piece-detail" d="M39.5 41.5c6.8-3.2 14.2-3.2 21 0" />
          <InkFoot narrow />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M31 15h9v7h6.5v-7h7v7H60v-7h9v13.5L64.5 34l3.5 42H32l3.5-42L31 28.5z"
          />
          <path className="piece-detail" d="M37 37h26M35.5 63h29" />
          <InkFoot />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M30.5 76c1-12 4.5-21 11-26.5-5-1-10-4-14-8-3-3-4-6-3-8.5l5.5-1-3.5-3c0-3 1.5-5.5 4.5-7 5-2.5 10.5-4 16-4.5l2-9 4.5 7.5 4.5-8 4 9.5c6 2.5 10.5 8 12.5 15.5 2.5 10 2 20.5-3 30l3 12.5z"
          />
          <path className="piece-detail" d="M58.5 25c4.8 6.7 6.8 15 6.1 24.9" />
          <circle className="piece-eye" cx="45.5" cy="26" r="2.6" />
          <InkFoot />
        </>
      );
    case "b":
      return (
        <>
          <circle className="piece-body" cx="50" cy="11" r="3.6" />
          <path
            className="piece-body"
            d="M50 16c8.5 5.5 13.5 13 13.5 21.5 0 7.5-3.5 13.5-9 17H45.5c-5.5-3.5-9-9.5-9-17 0-6 2.5-11.7 6.6-16.4l10.2 10.7 3.6-4.6-9.8-10.3c1-.9 2-1.7 2.9-2.4z"
          />
          <path
            className="piece-body"
            d="M41.5 54.5c1 6-1 11.5-5.5 16l-1.5 5.5h31l-1.5-5.5c-4.5-4.5-6.5-10-5.5-16z"
          />
          <path className="piece-detail" d="M40.5 61h19" />
          <InkFoot narrow />
        </>
      );
    case "q":
      return (
        <>
          <circle className="piece-body" cx="27" cy="13.5" r="2.7" />
          <circle className="piece-body" cx="38.5" cy="9.5" r="2.7" />
          <circle className="piece-body" cx="50" cy="8" r="2.7" />
          <circle className="piece-body" cx="61.5" cy="9.5" r="2.7" />
          <circle className="piece-body" cx="73" cy="13.5" r="2.7" />
          <path
            className="piece-body"
            d="M29 33 26 16.5l8.5 8L38 13l6.5 9.5L50 12l5.5 10.5L62 13l3.5 11.5 8.5-8L71 33z"
          />
          <path
            className="piece-body"
            d="M29.5 33h41l-3.5 8.5c-1.6 2.6-4 4.2-7 4.8 2.2 9.4 5 18.9 9.5 29.7H30.5c4.5-10.8 7.3-20.3 9.5-29.7-3-.6-5.4-2.2-7-4.8z"
          />
          <path className="piece-detail" d="M38 48c7.5 3 16.5 3 24 0" />
          <InkFoot />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M46.5 5h7v7.5H61v7h-7.5V27h-7v-7.5H39v-7h7.5z"
          />
          <path
            className="piece-body"
            d="M36.5 34.5h27l-2.5 9c-1.3 2.3-3.3 3.8-6 4.5 2 9.2 4.7 18.4 9 28H33c4.3-9.6 7-18.8 9-28-2.7-.7-4.7-2.2-6-4.5z"
          />
          <path className="piece-body" d="M38 27h24l-1.5 7.5h-21z" />
          <path className="piece-detail" d="M39 51.5h22" />
          <InkFoot />
        </>
      );
  }
}

/** Blueprint: shared drafting plinth — two square setbacks. */
function BlueprintPlinth() {
  return (
    <>
      <path
        className="piece-body piece-base"
        d="M30 74h40v6.5h6V87H24v-6.5h6z"
      />
      <path className="piece-detail" d="M30 80.5h40" />
    </>
  );
}

/** Schematic elevation language: hollow linework, construction hairlines. */
function BlueprintGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <circle className="piece-body" cx="50" cy="24" r="10.5" />
          <path className="piece-body" d="M43.5 34h13l5 40h-23z" />
          <path className="piece-detail" d="M41.5 56h17" />
          <path className="piece-hairline" d="M50 10v73" />
          <BlueprintPlinth />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M33 14h8v6.5h6v-6.5h6v6.5h6V14h8v11.5l-4 4L65.5 74h-31l2.5-44.5-4-4z"
          />
          <path className="piece-detail" d="M37 29.5h26M36 64.5h28" />
          <path className="piece-hairline" d="M50 14v69" />
          <BlueprintPlinth />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M32 74l3.5-16-9.5-12-3.5-8 7-1-4.5-4 5.5-7.5 14-5.5 2.5-9.5 4 6.5 4-6.5 4 7 9 6.5 4.5 14-1.5 20 3 16z"
          />
          <path className="piece-detail" d="M38 41l11 6M60 26l6 20" />
          <circle className="piece-eye" cx="45" cy="27" r="2.4" />
          <path className="piece-hairline" d="M52 10v73" />
          <BlueprintPlinth />
        </>
      );
    case "b":
      return (
        <>
          <circle className="piece-body" cx="50" cy="12" r="3.6" />
          <path
            className="piece-body"
            d="M50 17c7 6 11.5 12.5 11.5 20.5 0 7-3 12.5-8 16.5h-7c-5-4-8-9.5-8-16.5 0-8 4.5-14.5 11.5-20.5z"
          />
          <path className="piece-body" d="M43.5 54l-3 20h19l-3-20z" />
          <path className="piece-detail" d="M43 27.5l11 11.5M41.5 59.5h17" />
          <path className="piece-hairline" d="M50 8v75" />
          <BlueprintPlinth />
        </>
      );
    case "q":
      return (
        <>
          <path
            className="piece-body"
            d="M28.5 32.5 26 15l9.5 9 7-11.5 7.5 10 7.5-10 7 11.5 9.5-9-2.5 17.5z"
          />
          <path
            className="piece-body"
            d="M36 32.5h28l-4.5 11c4.5 9.5 6.5 19.5 7 30.5h-33c.5-11 2.5-21 7-30.5z"
          />
          <path className="piece-detail" d="M39 44h22M40.5 59.5h19" />
          <path className="piece-hairline" d="M50 12v71" />
          <BlueprintPlinth />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M46.5 4.5h7V12H61v7h-7.5v7.5h-7V19H39v-7h7.5z"
          />
          <path
            className="piece-body"
            d="M34.5 33h31l-2 8-4 5 5.5 28h-30l5.5-28-4-5z"
          />
          <path className="piece-detail" d="M36.5 41h27M40 61h20" />
          <path className="piece-hairline" d="M50 4.5v78.5" />
          <BlueprintPlinth />
        </>
      );
  }
}

/** Deco: shared ziggurat base — three square setbacks. */
function DecoBase() {
  return (
    <path
      className="piece-body piece-base"
      d="M35 70.5h30V76h7v5.5h7V87H21v-5.5h7V76h7z"
    />
  );
}

/** Brass-age geometry: stepped forms, fans, strong waists, tiered bases. */
function DecoGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <path
            className="piece-body"
            d="M50 14.5l7.4 3.1 3.1 7.4-3.1 7.4-7.4 3.1-7.4-3.1-3.1-7.4 3.1-7.4z"
          />
          <path
            className="piece-body"
            d="M43 36h14c-1 8 0 16 3.5 23.5l2.5 11H37l2.5-11C43 52 44 44 43 36z"
          />
          <path className="piece-accent" d="M43 50.5l7 5.5 7-5.5" />
          <DecoBase />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M42 13h16v10h7v9h3c-.5 12.5 0 25.5 1.5 38.5H30.5C32 57.5 32.5 44.5 32 32h3v-9h7z"
          />
          <path
            className="piece-detail"
            d="M43.5 51v13M50 47.5v16.5M56.5 51v13"
          />
          <path className="piece-accent" d="M36 37.5h28" />
          <DecoBase />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M31 70.5c1.5-9 4.5-16 10-21l-13.5-5-4.5-7.5 6.5-1.5-4.5-3.5 5-6.5 13.5-5 2-8.5 4.5 6 5-7 4 7.5 5 2v8h5v9h4v8c.5 9-1 17.5-4 25z"
          />
          <path className="piece-accent" d="M46.5 32c8 6.5 12.5 15 13.5 26" />
          <path className="piece-eye" d="M45.5 24l3.2 2.7-3.2 2.7-3.2-2.7z" />
          <DecoBase />
        </>
      );
    case "b":
      return (
        <>
          <path
            className="piece-body"
            d="M50 13c8 5.5 12.5 13 12.5 21.5 0 7-3 12.5-8 16.5h-9c-5-4-8-9.5-8-16.5C37.5 26 42 18.5 50 13z"
          />
          <path
            className="piece-accent"
            d="M50 20.5V50M39.5 27 50 50l10.5-23"
          />
          <path className="piece-body" d="M40.5 51h19l1.5 6h-22z" />
          <path className="piece-body" d="M42.5 57l-2.5 13.5h20L57.5 57z" />
          <DecoBase />
        </>
      );
    case "q":
      return (
        <>
          <path
            className="piece-body"
            d="M30 33l-3.5-17.5 9 9.5 6.5-13.5 5 11.5 3-13.5 3 13.5 5-11.5 6.5 13.5 9-9.5L70 33z"
          />
          <path
            className="piece-body"
            d="M31.5 33h37c.5 4.5-1.5 8-5.5 10l1.5 4.5c-2 8 0 15 4.5 23H31c4.5-8 6.5-15 4.5-23l1.5-4.5c-4-2-6-5.5-5.5-10z"
          />
          <path
            className="piece-accent"
            d="M50 28.5v-12M42 29l-4.5-10.5M58 29l4.5-10.5"
          />
          <DecoBase />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M46 5h8v7.5h7.5v8H54V28h-8v-7.5h-7.5v-8H46z"
          />
          <path className="piece-body" d="M37 28h26v6h-5v5H42v-5h-5z" />
          <path
            className="piece-body"
            d="M38.5 39h23c1 7-1 14.5 2.5 22l3 9.5H33l3-9.5c3.5-7.5 1.5-15 2.5-22z"
          />
          <path className="piece-accent" d="M50 43v24" />
          <DecoBase />
        </>
      );
  }
}

/**
 * Shared horse: left-facing muzzle, ear, jaw, haunch, then each set's foot.
 * One silhouette so the knight reads as a knight; identity lives in cut and
 * proportion, not in a different blob.
 */
function OrganicKnight({ extraDetail }: { extraDetail?: ReactNode }) {
  return (
    <>
      <path
        className="piece-body"
        d="M32 76c0-12 6-22 14-28C36 46 24 40 18 32l-2-4c0-4 3-8 8-9l2-5c-1-4 2-8 8-7l6-6 6 8c10 2 16 12 16 22 0 9-6 16-14 20 2 6 4 12 5 19z"
      />
      <path
        className="piece-detail"
        d="M50 18c8 8 10 18 8 30M20 30c6 3 12 5 18 5M24 24c3 1 6 1 8 0"
      />
      <circle className="piece-eye" cx="34" cy="23" r="2.4" />
      {extraDetail}
    </>
  );
}

/** Horn: lathe-turned ivory — collars, merlons, a carved horse. */
function HornFoot() {
  return (
    <>
      <path
        className="piece-body"
        d="M32 73h36c3.5 2.2 5 4.6 5 7H27c0-2.4 1.5-4.8 5-7z"
      />
      <path className="piece-body piece-base" d="M24 80h52l6 10H18z" />
    </>
  );
}

function HornGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <circle className="piece-body" cx="50" cy="20" r="12.5" />
          <path
            className="piece-body"
            d="M36 33h28a4 4 0 0 1 0 8H36a4 4 0 0 1 0-8z"
          />
          <path
            className="piece-body"
            d="M42 41c1 12-2 22-8 32h32c-6-10-9-20-8-32z"
          />
          <path className="piece-detail" d="M38 52h24M34 64h32" />
          <HornFoot />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M25 11h11v9h6V11h7v9h6V11h14v16l-6 6c1 12 2 24 2 37H29c0-13 1-25 2-37l-6-6z"
          />
          <path className="piece-detail" d="M31 31h38M33 46h34M31 62h38" />
          <HornFoot />
        </>
      );
    case "n":
      return (
        <>
          <OrganicKnight />
          <HornFoot />
        </>
      );
    case "b":
      return (
        <>
          <circle className="piece-body" cx="50" cy="10" r="3.8" />
          <path
            className="piece-body"
            d="M50 14c11 9 17 20 17 32 0 8-4 14-10 18H43c-6-4-10-10-10-18 0-12 6-23 17-32z"
          />
          <path
            className="piece-detail"
            d="M50 22v36M42 28l8 14 8-14M38 56h24"
          />
          <path
            className="piece-body"
            d="M38 64h24c-1 4-3 7-7 9H45c-4-2-6-5-7-9z"
          />
          <HornFoot />
        </>
      );
    case "q":
      return (
        <>
          <circle className="piece-body" cx="22" cy="16" r="3.6" />
          <circle className="piece-body" cx="36" cy="9" r="3.6" />
          <circle className="piece-body" cx="50" cy="7" r="3.6" />
          <circle className="piece-body" cx="64" cy="9" r="3.6" />
          <circle className="piece-body" cx="78" cy="16" r="3.6" />
          <path
            className="piece-body"
            d="M24 20 32 38 36 16 44 36 50 14 56 36 64 16 68 38 76 20 70 42H30z"
          />
          <path
            className="piece-body"
            d="M32 42h36c1 5-1 9-5 12 3 8 5 16 8 25H29c3-9 5-17 8-25-4-3-6-7-5-12z"
          />
          <path className="piece-detail" d="M36 50h28M34 62h32" />
          <HornFoot />
        </>
      );
    case "k":
      return (
        <>
          <path className="piece-body" d="M46 5h8v9h9v8h-9v10h-8V22h-9v-8h9z" />
          <path
            className="piece-body"
            d="M36 32h28a3.5 3.5 0 0 1 0 7H36a3.5 3.5 0 0 1 0-7z"
          />
          <path
            className="piece-body"
            d="M38 39c-4 6-5 12-3 18 1 5 4 9 8 12-2 5-4 10-6 16h38c-2-6-4-11-6-16 4-3 7-7 8-12 2-6 1-12-3-18z"
          />
          <path className="piece-detail" d="M36 52h28M40 64h20" />
          <HornFoot />
        </>
      );
  }
}

/** Nib: printed chessmen with interior cuts — not the tiny SAN glyph. */
function NibGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <path
            className="piece-body"
            d="M50 9a12 12 0 0 1 7 22c9 5 14 15 15 30v12H28V61c1-15 6-25 15-30a12 12 0 0 1 7-22z"
          />
          <path className="piece-detail" d="M38 34c8-4 16-4 24 0M36 52h28" />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M22 10h13v10h8V10h14v10h8V10h13v22l-8 7v32l8 8v13H22V79l8-8V39l-8-7z"
          />
          <path className="piece-detail" d="M32 36h36M30 58h40M28 76h44" />
        </>
      );
    case "n":
      return (
        <OrganicKnight
          extraDetail={
            <path className="piece-detail" d="M32 48c8 4 16 6 24 4M28 82h40" />
          }
        />
      );
    case "b":
      return (
        <>
          <path
            className="piece-body"
            d="M50 6c14 12 22 26 22 40 0 10-5 18-13 23l5 10h14v15H22V79h14l5-10c-8-5-13-13-13-23 0-14 8-28 22-40z"
          />
          <path
            className="piece-detail"
            d="M50 18v44M40 28l10 16 10-16M38 58h24"
          />
        </>
      );
    case "q":
      return (
        <>
          <path
            className="piece-body"
            d="M50 5 60 28 80 12 74 42c8 4 12 12 10 22l-4 16h8v14H12V80h8l-4-16c-2-10 2-18 10-22L20 12l20 16z"
          />
          <path className="piece-detail" d="M34 48c10 5 22 5 32 0M30 64h40" />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M44 5h12v12h12v11H56v8c14 2 22 12 22 26l-3 18h9v14H16V80h9l-3-18c0-14 8-24 22-26v-8H32V17h12z"
          />
          <path className="piece-detail" d="M36 48h28M34 64h32" />
        </>
      );
  }
}

/** Lamp: tall barley-sugar turning — extra collars, a slender horse. */
function LampFoot() {
  return (
    <>
      <path
        className="piece-body"
        d="M34 78h32c2.8 1.6 4 3.4 4 5.4H30c0-2 1.2-3.8 4-5.4z"
      />
      <path className="piece-body piece-base" d="M26 83.5h48l4 8.5H22z" />
    </>
  );
}

function LampGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <circle className="piece-body" cx="50" cy="16" r="10" />
          <path
            className="piece-body"
            d="M39 27h22a3 3 0 0 1 0 6H39a3 3 0 0 1 0-6z"
          />
          <path
            className="piece-body"
            d="M44 33c0 16-2 30-6 45h24c-4-15-6-29-6-45z"
          />
          <path className="piece-detail" d="M42 44h16M40 58h20M38 70h24" />
          <LampFoot />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M29 6h9v7h6V6h12v7h6V6h9v14l-4 5c1 14 1 30 1 48H32c0-18 0-34 1-48l-4-5z"
          />
          <path
            className="piece-detail"
            d="M33 24h34M34 40h32M33 58h34M32 72h36"
          />
          <LampFoot />
        </>
      );
    case "n":
      return (
        <>
          <g transform="translate(50 80) scale(0.88 1.12) translate(-50 -80)">
            <OrganicKnight />
          </g>
          <LampFoot />
        </>
      );
    case "b":
      return (
        <>
          <circle className="piece-body" cx="50" cy="8" r="3.2" />
          <path
            className="piece-body"
            d="M50 12c9 11 13 23 13 36 0 8-3 14-8 18H45c-5-4-8-10-8-18 0-13 4-25 13-36z"
          />
          <path
            className="piece-detail"
            d="M50 20v40M43 28l7 14 7-14M40 54h20"
          />
          <path
            className="piece-body"
            d="M40 66h20c-1 4-3 8-6 10H46c-3-2-5-6-6-10z"
          />
          <LampFoot />
        </>
      );
    case "q":
      return (
        <>
          <circle className="piece-body" cx="26" cy="14" r="2.8" />
          <circle className="piece-body" cx="38" cy="7" r="2.8" />
          <circle className="piece-body" cx="50" cy="5" r="2.8" />
          <circle className="piece-body" cx="62" cy="7" r="2.8" />
          <circle className="piece-body" cx="74" cy="14" r="2.8" />
          <path
            className="piece-body"
            d="M28 18 34 36 38 14 44 34 50 12 56 34 62 14 66 36 72 18 68 40H32z"
          />
          <path
            className="piece-body"
            d="M34 40h32c0 6-2 10-6 13 2 10 4 20 7 27H33c3-7 5-17 7-27-4-3-6-7-6-13z"
          />
          <path className="piece-detail" d="M38 48h24M36 60h28M34 72h32" />
          <LampFoot />
        </>
      );
    case "k":
      return (
        <>
          <path className="piece-body" d="M46 3h8v8h8v7h-8v9h-8V18h-8v-7h8z" />
          <path
            className="piece-body"
            d="M38 27h24a3 3 0 0 1 0 6H38a3 3 0 0 1 0-6z"
          />
          <path
            className="piece-body"
            d="M40 33c-3 8-4 16-2 24 1 5 4 9 8 12-2 6-4 12-5 19h30c-1-7-3-13-5-19 4-3 7-7 8-12 2-8 1-16-2-24z"
          />
          <path className="piece-detail" d="M38 48h24M40 62h20M38 74h24" />
          <LampFoot />
        </>
      );
  }
}

/** Foundry: milled planes — chamfers, rebates, a constructed horse. */
function FoundryPlinth() {
  return (
    <>
      <path className="piece-body" d="M30 72h40v5H30z" />
      <path className="piece-body" d="M26 77h48v5H26z" />
      <path className="piece-body piece-base" d="M20 82h60v10H20z" />
    </>
  );
}

function FoundryGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <path
            className="piece-body"
            d="M50 8 62 15l5 12-5 12-12 7-12-7-5-12 5-12z"
          />
          <path className="piece-body" d="M36 44h28l-2 6H38z" />
          <path className="piece-body" d="M40 50h20l4 22H36z" />
          <path className="piece-detail" d="M50 15v34M38 44h24M42 58h16" />
          <FoundryPlinth />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M24 8h12v11h7V8h14v11h7V8h12v18H24z"
          />
          <path className="piece-body" d="M28 26h44l-3 8H31z" />
          <path className="piece-body" d="M32 34h36l4 38H28z" />
          <path
            className="piece-detail"
            d="M24 26h52M31 34h38M34 50h32M32 64h36"
          />
          <FoundryPlinth />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M32 76 30 62 40 52 26 42 18 34 20 24 30 20 32 10 42 8 50 14 54 8 62 16 72 28 74 44 66 58 58 64 58 76z"
          />
          <path
            className="piece-detail"
            d="M32 20 48 34M20 30h14M50 12 64 28M36 50h22"
          />
          <path className="piece-eye" d="M34 22h5v5h-5z" />
          <FoundryPlinth />
        </>
      );
    case "b":
      return (
        <>
          <path className="piece-body" d="M50 6 58 18 50 30 42 18z" />
          <path className="piece-body" d="M36 30h28l-4 16H40z" />
          <path className="piece-body" d="M38 46h24l4 26H34z" />
          <path
            className="piece-detail"
            d="M50 10v52M42 30h16M40 46h20M38 60h24"
          />
          <FoundryPlinth />
        </>
      );
    case "q":
      return (
        <>
          <path
            className="piece-body"
            d="M24 26 30 8 42 22 50 6 58 22 70 8 76 26 68 34H32z"
          />
          <path className="piece-body" d="M32 34h36l-3 8H35z" />
          <path className="piece-body" d="M36 42h28l4 30H32z" />
          <path
            className="piece-detail"
            d="M32 34h36M38 42h24M36 56h28M34 66h32"
          />
          <FoundryPlinth />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M44 4h12v9h9v10h-9v10H44V23h-9V13h9z"
          />
          <path className="piece-body" d="M34 33h32l-3 7H37z" />
          <path className="piece-body" d="M36 40h28l4 32H32z" />
          <path
            className="piece-detail"
            d="M34 33h32M38 40h24M36 54h28M34 66h32"
          />
          <FoundryPlinth />
        </>
      );
  }
}

/** Relay: telephone exchange coils — winding ribs, ceramic insulator discs. */
function RelayInsulatorBase() {
  return (
    <>
      <path
        className="piece-body"
        d="M34 70h32c2.5 1.5 3.5 3 3.5 4.5H30.5c0-1.5 1-3 3.5-4.5z"
      />
      <path
        className="piece-body"
        d="M30 75h40c3 1.8 4.5 3.6 4.5 5.5H25.5c0-1.9 1.5-3.7 4.5-5.5z"
      />
      <path
        className="piece-body piece-base"
        d="M24 81h52c4 2.5 5.5 5 5.5 8H18.5c0-3 1.5-5.5 5.5-8z"
      />
      <path className="piece-detail" d="M32 72.5h36M28 78h44" />
    </>
  );
}

function RelayGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <circle className="piece-body" cx="50" cy="18" r="11" />
          <path
            className="piece-body"
            d="M36 31h28a3.5 3.5 0 0 1 0 7H36a3.5 3.5 0 0 1 0-7z"
          />
          <path
            className="piece-body"
            d="M42 38c1 11-2 21-8 32h32c-6-11-9-21-8-32z"
          />
          <path className="piece-detail" d="M38 48h24M35 59h30" />
          <RelayInsulatorBase />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M26 12h11v9h6v-9h14v9h6v-9h11v15l-5 6c1 11 1 24 2 37H29c1-13 1-26 2-37l-5-6z"
          />
          <path
            className="piece-detail"
            d="M30 33h40M33 43h34M31 53h38M29 63h42"
          />
          <RelayInsulatorBase />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M33 70c-7-8-7-18 2-25-4 0-11-4-14-11-1-5 2-8 6-10 5-2 12-5 15-9l3-8 4 6 4-6 4 7c10 6 16 20 13 36-2 7-3 14-3 20z"
          />
          <path
            className="piece-detail"
            d="M35 45c4-3 7-8 7-14M23 33c3 0 5-1 7-3M52 24h13M56 34h15M54 44h16M50 54h18M46 64h18"
          />
          <circle className="piece-eye" cx="41" cy="22" r="2.8" />
          <RelayInsulatorBase />
        </>
      );
    case "b":
      return (
        <>
          <circle className="piece-body" cx="50" cy="9" r="3.5" />
          <path
            className="piece-body"
            d="M50 13c12 10 18 21 18 33 0 7-4 13-10 17H42c-6-4-10-10-10-17 0-12 6-23 18-33z"
          />
          <path
            className="piece-body"
            d="M39 63h22c-1 3-3 5-6 7H45c-3-2-5-4-6-7z"
          />
          <path
            className="piece-detail"
            d="M50 20v38M41 27l9 14 9-14M37 54h26M39 60h22"
          />
          <RelayInsulatorBase />
        </>
      );
    case "q":
      return (
        <>
          <circle className="piece-body" cx="22" cy="16" r="3.5" />
          <circle className="piece-body" cx="36" cy="9" r="3.5" />
          <circle className="piece-body" cx="50" cy="7" r="3.5" />
          <circle className="piece-body" cx="64" cy="9" r="3.5" />
          <circle className="piece-body" cx="78" cy="16" r="3.5" />
          <path
            className="piece-body"
            d="M24 20 32 38 36 15 44 36 50 13 56 36 64 15 68 38 76 20 70 42H30z"
          />
          <path
            className="piece-body"
            d="M32 42h36c1 5-1 9-5 12 3 7 5 15 7 24H29c2-9 4-17 7-24-4-3-6-7-4-12z"
          />
          <path className="piece-detail" d="M35 49h30M33 59h34M31 69h38" />
          <RelayInsulatorBase />
        </>
      );
    case "k":
      return (
        <>
          <path className="piece-body" d="M46 5h8v8h8v8h-8v9h-8V21h-8v-8h8z" />
          <path
            className="piece-body"
            d="M36 30h28a3.5 3.5 0 0 1 0 7H36a3.5 3.5 0 0 1 0-7z"
          />
          <path
            className="piece-body"
            d="M39 37h22c4 7 5 14 2 20-2 5-5 8-9 11H46c-4-3-7-6-9-11-3-6-2-13 2-20z"
          />
          <path className="piece-detail" d="M38 49h24M40 60h20" />
          <RelayInsulatorBase />
        </>
      );
  }
}

/** Switchgear: enamel contacts and porcelain stacks with nickel ribs. */
function SwitchgearInsulatorBase() {
  return (
    <>
      <path className="piece-body" d="M34 68h32l5 5-5 5H34l-5-5z" />
      <path className="piece-body" d="M28 78h44l5 5-5 5H28l-5-5z" />
      <path className="piece-body piece-base" d="M20 88h60l4 5H16z" />
      <path className="piece-detail" d="M31 73h38M25 83h50" />
    </>
  );
}

function SwitchgearGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <circle className="piece-body" cx="50" cy="15" r="9.5" />
          <path className="piece-body" d="M38 26h24l4 4-4 4H38l-4-4z" />
          <path className="piece-body" d="M42 34h16l4 34H38z" />
          <path className="piece-detail" d="M39 42h22M38 50h24M37 58h26" />
          <SwitchgearInsulatorBase />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M24 10h13v11h7V10h12v11h7V10h13v17l-6 6H30l-6-6z"
          />
          <path className="piece-body" d="M33 33h34l4 35H29z" />
          <path
            className="piece-detail"
            d="M30 33h40M32 42h36M31 51h38M30 60h40"
          />
          <SwitchgearInsulatorBase />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M31 68c-6-9-5-17 3-24-6-1-13-5-16-11-2-5 1-9 7-12l13-5 4-10 7 9 7-4 2 9c10 7 15 19 13 32-1 6-4 11-8 16z"
          />
          <path
            className="piece-detail"
            d="M34 44c5-3 8-8 8-14M20 31c6 2 12 1 17-2M52 23h12M55 31h13M56 39h14M55 47h15M52 55h16M47 63h18"
          />
          <circle className="piece-eye" cx="40" cy="22" r="2.6" />
          <SwitchgearInsulatorBase />
        </>
      );
    case "b":
      return (
        <>
          <circle className="piece-body" cx="50" cy="8" r="3.5" />
          <path
            className="piece-body"
            d="M50 12c12 9 18 19 18 29 0 7-4 12-10 16H42c-6-4-10-9-10-16 0-10 6-20 18-29z"
          />
          <path className="piece-body" d="M38 57h24l4 5-4 6H38l-4-6z" />
          <path
            className="piece-detail"
            d="M50 17v35M40 25l10 15 10-15M37 48h26M36 62h28"
          />
          <SwitchgearInsulatorBase />
        </>
      );
    case "q":
      return (
        <>
          <circle className="piece-body" cx="24" cy="15" r="3.5" />
          <circle className="piece-body" cx="37" cy="9" r="3.5" />
          <circle className="piece-body" cx="50" cy="7" r="3.5" />
          <circle className="piece-body" cx="63" cy="9" r="3.5" />
          <circle className="piece-body" cx="76" cy="15" r="3.5" />
          <path
            className="piece-body"
            d="M25 19 32 37 37 14 44 35 50 12 56 35 63 14 68 37 75 19 69 41H31z"
          />
          <path
            className="piece-body"
            d="M34 41h32l4 6-6 6 4 15H32l4-15-6-6z"
          />
          <path className="piece-detail" d="M33 47h34M35 55h30M33 63h34" />
          <SwitchgearInsulatorBase />
        </>
      );
    case "k":
      return (
        <>
          <path className="piece-body" d="M46 4h8v9h9v8h-9v9h-8v-9h-9v-8h9z" />
          <path className="piece-body" d="M37 30h26l5 4-5 5H37l-5-5z" />
          <path className="piece-body" d="M40 39h20l6 29H34z" />
          <path className="piece-detail" d="M36 47h28M35 55h30M34 63h32" />
          <SwitchgearInsulatorBase />
        </>
      );
  }
}

/** Chisel: cyclopean stone — planar facets, triangular bevels, block plinth. */
function ChiselPlinth() {
  return (
    <>
      <path className="piece-body" d="M32 72h36l-2 6H34z" />
      <path className="piece-body" d="M28 78h44l-2 5H30z" />
      <path className="piece-body piece-base" d="M22 83h56l4 8H18z" />
      <path
        className="piece-detail"
        d="M34 72l-4 6M66 72l4 6M30 78l-4 5M70 78l4 5"
      />
    </>
  );
}

function ChiselGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <circle className="piece-body" cx="50" cy="20" r="11" />
          <path className="piece-body" d="M36 33h28l-3 8H39z" />
          <path className="piece-body" d="M41 41h18l4 31H37z" />
          <path className="piece-detail" d="M40 22h20M50 41v31M42 56l8 6 8-6" />
          <ChiselPlinth />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M24 10h12v12h6V10h16v12h6V10h12v18l-4 8H28l-4-8z"
          />
          <path className="piece-body" d="M28 36h44l4 36H24z" />
          <path
            className="piece-detail"
            d="M24 28h52M28 36h44M33 50h34M28 62h44M50 36v36"
          />
          <ChiselPlinth />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M32 72 25 56 33 44 21 38 17 30 23 24 38 18 42 7 47 13 52 7 56 15 68 25 73 40 71 56 68 72z"
          />
          <path
            className="piece-detail"
            d="M33 44 44 32M23 24 35 30M47 13 62 28M56 15 70 38M60 38 71 52M36 62h30"
          />
          <path className="piece-eye" d="M40 19l3.5 3-3.5 3-3.5-3z" />
          <ChiselPlinth />
        </>
      );
    case "b":
      return (
        <>
          <path
            className="piece-body"
            d="M50 6 60 16 66 30 63 42 56 51H44l-7-9-3-12 6-14z"
          />
          <path className="piece-body" d="M36 51h28l-3 8H39z" />
          <path className="piece-body" d="M40 59h20l6 13H34z" />
          <path
            className="piece-detail"
            d="M42 36 59 15M37 30l13 14 13-14M39 64h22"
          />
          <ChiselPlinth />
        </>
      );
    case "q":
      return (
        <>
          <path
            className="piece-body"
            d="M22 24 28 8 39 20 50 6 61 20 72 8 78 24 70 34H30z"
          />
          <path className="piece-body" d="M30 34h40l-4 10H34z" />
          <path className="piece-body" d="M34 44h32l5 28H29z" />
          <path
            className="piece-detail"
            d="M39 20v14M50 6v28M61 20v14M34 44l16 10 16-10M37 59h26"
          />
          <ChiselPlinth />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M44 4h12v9h9v11h-9v10H44V24h-9V13h9z"
          />
          <path className="piece-body" d="M34 34h32l-3 8H37z" />
          <path className="piece-body" d="M36 42h28l5 30H31z" />
          <path
            className="piece-detail"
            d="M50 4v30M34 34h32M37 42h26M39 56l11 7 11-7M35 66h30"
          />
          <ChiselPlinth />
        </>
      );
  }
}

/** Kiln: pressed stoneware with chipped planes and sgraffito cuts. */
function KilnPlinth() {
  return (
    <>
      <path className="piece-body" d="M32 72h36l-2 6H34z" />
      <path className="piece-body" d="M28 78h44l-2 5H30z" />
      <path className="piece-body piece-base" d="M22 83h56l4 8H18z" />
      <path
        className="piece-detail"
        d="M34 72l-4 6M66 72l4 6M30 78l-4 5M70 78l4 5"
      />
    </>
  );
}

function KilnGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <circle className="piece-body" cx="50" cy="20" r="11" />
          <path className="piece-body" d="M36 33h28l-3 8H39z" />
          <path className="piece-body" d="M41 41h18l4 31H37z" />
          <path className="piece-detail" d="M41 22h18M50 41v31M42 56l8 6 8-6" />
          <KilnPlinth />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M24 10h12v12h6V10h16v12h6V10h12v18l-4 8H28l-4-8z"
          />
          <path className="piece-body" d="M28 36h44l4 36H24z" />
          <path
            className="piece-detail"
            d="M24 28h52M28 36h44M33 50h34M28 62h44M50 36v36"
          />
          <KilnPlinth />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M32 72 26 59 33 47 24 43 18 37 16 29 23 23 37 18 42 6 49 14 55 11 56 17 67 25 73 39 72 54 66 72z"
          />
          <path
            className="piece-detail"
            d="M20 30 34 32 28 40M34 47 45 34M43 15 57 28M56 17 54 29 67 26 61 38 72 39M58 40 71 53M36 62h30"
          />
          <path className="piece-eye" d="M39 20l3.5 3-3.5 3-3.5-3z" />
          <KilnPlinth />
        </>
      );
    case "b":
      return (
        <>
          <path
            className="piece-body"
            d="M50 6c11 9 17 20 17 31 0 8-4 14-11 18H44c-7-4-11-10-11-18 0-11 6-22 17-31z"
          />
          <path className="piece-body" d="M37 55h26l-2 7H39z" />
          <path className="piece-body" d="M40 62h20l5 10H35z" />
          <path
            className="piece-detail"
            d="M42 38 59 15M39 30l11 14 11-14M39 67h22"
          />
          <KilnPlinth />
        </>
      );
    case "q":
      return (
        <>
          <path
            className="piece-body"
            d="M22 24 28 8 39 20 50 6 61 20 72 8 78 24 70 34H30z"
          />
          <path className="piece-body" d="M30 34h40l-4 10H34z" />
          <path className="piece-body" d="M34 44h32l5 28H29z" />
          <path
            className="piece-detail"
            d="M39 20v14M50 6v28M61 20v14M34 44l16 10 16-10M37 59h26"
          />
          <KilnPlinth />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M44 4h12v9h9v11h-9v10H44V24h-9V13h9z"
          />
          <path className="piece-body" d="M34 34h32l-3 8H37z" />
          <path className="piece-body" d="M36 42h28l5 30H31z" />
          <path
            className="piece-detail"
            d="M50 4v30M34 34h32M37 42h26M39 56l11 7 11-7M35 66h30"
          />
          <KilnPlinth />
        </>
      );
  }
}

/** Matrix: hot-metal typography — type sorts, ink traps, punch-cut serifs. */
function MatrixPlinth() {
  return (
    <>
      <path className="piece-body" d="M30 73h40v5H30z" />
      <path className="piece-body piece-base" d="M24 78h52v10H24z" />
      <path className="piece-detail" d="M30 73v5M70 73v5M28 83h44" />
      <path className="piece-detail" d="M24 82h6v2h-6z" />
    </>
  );
}

function MatrixGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <path
            className="piece-body"
            d="M50 12a11 11 0 0 1 6 20.5c7 4.5 11 12.5 12 23.5H32c1-11 5-19 12-23.5A11 11 0 0 1 50 12z"
          />
          <path className="piece-body" d="M32 56h36l2 17H30z" />
          <path
            className="piece-detail"
            d="M41 38c6-2.5 12-2.5 18 0M36 62h28M50 12v12"
          />
          <MatrixPlinth />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M26 12h12v8h6v-8h12v8h6v-8h12v16l-5 6v31H31V28l-5-6z"
          />
          <path
            className="piece-detail"
            d="M31 34h38M34 44h32M31 54h38M28 64h44M50 20v45"
          />
          <MatrixPlinth />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M30 73c-6-8-6-18 3-26-4 0-11-4-14-11-1-5 2-8 6-10 5-2 12-5 15-9l3-7 4 6 4-6 4 7c10 6 16 20 13 36-1 7-2 13-3 20z"
          />
          <path
            className="piece-detail"
            d="M33 47c4-3 7-8 7-14M21 35c3 0 5-1 7-3M54 20c8 6 11 16 9 28M58 36c6 5 8 13 6 22M34 62h32"
          />
          <circle className="piece-eye" cx="41" cy="22" r="2.8" />
          <MatrixPlinth />
        </>
      );
    case "b":
      return (
        <>
          <circle className="piece-body" cx="50" cy="10" r="3.4" />
          <path
            className="piece-body"
            d="M50 14c10 7 15 15 15 25 0 8-4 14-10 18H45c-6-4-10-10-10-18 0-10 5-18 15-25z"
          />
          <path className="piece-body" d="M37 57h26l3 16H34z" />
          <path
            className="piece-detail"
            d="M50 18v39M42 27l8 12 8-12M38 64h24"
          />
          <MatrixPlinth />
        </>
      );
    case "q":
      return (
        <>
          <circle className="piece-body" cx="25" cy="15" r="2.8" />
          <circle className="piece-body" cx="37" cy="10" r="2.8" />
          <circle className="piece-body" cx="50" cy="8" r="2.8" />
          <circle className="piece-body" cx="63" cy="10" r="2.8" />
          <circle className="piece-body" cx="75" cy="15" r="2.8" />
          <path
            className="piece-body"
            d="M26 18 33 34 38 14 44 32 50 12 56 32 62 14 67 34 74 18 69 40H31z"
          />
          <path
            className="piece-body"
            d="M31 40h38l-4 10c4 7 6 15 7 23H28c1-8 3-16 7-23z"
          />
          <path className="piece-detail" d="M36 48h28M34 60h32" />
          <MatrixPlinth />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M45 4h10v9h10v9h-10v10H45V22h-10v-9h10z"
          />
          <path className="piece-body" d="M35 32h30l-3 8H38z" />
          <path className="piece-body" d="M36 40h28l4 33H32z" />
          <path
            className="piece-detail"
            d="M35 32h30M38 40h24M36 52h28M34 64h32"
          />
          <MatrixPlinth />
        </>
      );
  }
}

/** Compositor: slab-terminal type sorts with oversized ink traps. */
function CompositorPlinth() {
  return (
    <>
      <path className="piece-body" d="M30 69h40v7H30z" />
      <path className="piece-body piece-base" d="M23 76h54v14H23z" />
      <path className="piece-detail" d="M30 69v7M70 69v7M27 82h46" />
      <path className="piece-detail" d="M23 80h6v3h-6M71 80h6v3h-6" />
    </>
  );
}

function CompositorGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <path className="piece-body" d="M42 11h16l7 7v10l-7 7H42l-7-7V18z" />
          <path className="piece-body" d="M36 38h28v8H36z" />
          <path className="piece-body" d="M41 46h18l5 23H36z" />
          <path className="piece-detail" d="M42 31h16M39 58h22M50 11v10" />
          <CompositorPlinth />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M24 10h14v11h6V10h12v11h6V10h14v19H24z"
          />
          <path className="piece-body" d="M29 29h42v8H29z" />
          <path className="piece-body" d="M33 37h34l3 32H30z" />
          <path
            className="piece-detail"
            d="M29 29h42M32 45h36M31 56h38M30 65h40M50 21v48"
          />
          <CompositorPlinth />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M30 69 28 60 36 47 27 44 18 37 18 29 25 24 38 20 42 8 49 16 56 11 57 19 66 26 72 39 69 55 62 69z"
          />
          <path
            className="piece-detail"
            d="M21 31h18l-9 10M36 47 47 35M56 20l8 7-7 3 10 5-8 3 9 6-10 2M36 56h31M32 63h33"
          />
          <path className="piece-eye" d="M38 23h5v5h-5z" />
          <CompositorPlinth />
        </>
      );
    case "b":
      return (
        <>
          <path
            className="piece-body"
            d="M50 7 64 23 60 39 55 45H45l-5-6-4-16z"
          />
          <path className="piece-body" d="M37 45h26v8H37z" />
          <path className="piece-body" d="M40 53h20l5 16H35z" />
          <path
            className="piece-detail"
            d="M42 31 58 15M41 23l9 16 9-16M38 61h24"
          />
          <CompositorPlinth />
        </>
      );
    case "q":
      return (
        <>
          <path className="piece-body" d="M20 11h7v7h-7z" />
          <path className="piece-body" d="M34 7h7v7h-7z" />
          <path className="piece-body" d="M47 5h7v7h-7z" />
          <path className="piece-body" d="M60 7h7v7h-7z" />
          <path className="piece-body" d="M73 11h7v7h-7z" />
          <path
            className="piece-body"
            d="M23 18 31 36 37 14 44 34 50 12 56 34 63 14 69 36 77 18 70 41H30z"
          />
          <path className="piece-body" d="M31 41h38v8H31z" />
          <path className="piece-body" d="M36 49h28l5 20H31z" />
          <path className="piece-detail" d="M31 41h38M34 58h32M50 12v29" />
          <CompositorPlinth />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M44 4h12v9h10v10H56v10H44V23H34V13h10z"
          />
          <path className="piece-body" d="M34 33h32v8H34z" />
          <path className="piece-body" d="M38 41h24l6 28H32z" />
          <path
            className="piece-detail"
            d="M34 33h32M36 49h28M34 59h32M32 66h36M50 41v28"
          />
          <CompositorPlinth />
        </>
      );
  }
}

/** Optic: optical bench — diopter apertures, bold high-speed silhouette geometry. */
function OpticBase() {
  return (
    <>
      <path
        className="piece-body"
        d="M33 74h34c2.5 1.5 3.5 3 3.5 4.5H29.5c0-1.5 1-3 3.5-4.5z"
      />
      <path className="piece-body piece-base" d="M26 79h48l4 9H22z" />
      <path className="piece-detail" d="M31 77h38M28 84h44" />
    </>
  );
}

function OpticGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <circle className="piece-body" cx="50" cy="18" r="11" />
          <path
            className="piece-body"
            d="M38 29h24a3 3 0 0 1 0 6H38a3 3 0 0 1 0-6z"
          />
          <path
            className="piece-body"
            d="M43 35c0 14-2 27-6 39h26c-4-12-6-25-6-39z"
          />
          <circle className="piece-eye" cx="50" cy="18" r="3.5" />
          <path className="piece-detail" d="M41 48h18M39 60h22" />
          <OpticBase />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            d="M27 10h12v9h6v-9h10v9h6v-9h12v16l-5 6c1 13 1 27 2 42H30c1-15 1-29 2-42l-5-6z"
          />
          <path
            className="piece-detail"
            d="M31 31h38M33 46h34M31 60h38M30 68h40"
          />
          <OpticBase />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            d="M33 74c-7-9-7-19 2-28-4 0-11-4-14-11-1-5 2-8 6-10 5-2 12-5 15-9l3-8 4 6 4-6 4 7c10 6 16 20 13 36-2 7-3 15-3 23z"
          />
          <path
            className="piece-detail"
            d="M35 46c4-3 7-8 7-14M23 34c3 0 6-1 8-3M56 22c6 6 8 15 6 24M36 58h26"
          />
          <circle className="piece-eye" cx="41" cy="22" r="3.2" />
          <OpticBase />
        </>
      );
    case "b":
      return (
        <>
          <circle className="piece-body" cx="50" cy="9" r="3.6" />
          <circle className="piece-eye" cx="50" cy="9" r="1.5" />
          <path
            className="piece-body"
            d="M50 13c10 9 15 20 15 32 0 7-3 13-8 17H43c-5-4-8-10-8-17 0-12 5-23 15-32z"
          />
          <path
            className="piece-body"
            d="M39 62h22c-1 3-3 6-6 8H45c-3-2-5-5-6-8z"
          />
          <path
            className="piece-detail"
            d="M50 18v38M42 27l8 12 8-12M38 52h24"
          />
          <OpticBase />
        </>
      );
    case "q":
      return (
        <>
          <circle className="piece-body" cx="24" cy="16" r="3.2" />
          <circle className="piece-body" cx="37" cy="9" r="3.2" />
          <circle className="piece-body" cx="50" cy="7" r="3.2" />
          <circle className="piece-body" cx="63" cy="9" r="3.2" />
          <circle className="piece-body" cx="76" cy="16" r="3.2" />
          <path
            className="piece-body"
            d="M26 19 33 36 38 14 44 34 50 12 56 34 62 14 67 36 74 19 69 40H31z"
          />
          <path
            className="piece-body"
            d="M33 40h34c1 5-1 9-5 12 3 7 5 15 7 22H31c2-7 4-15 7-22-4-3-6-7-5-12z"
          />
          <path className="piece-detail" d="M36 48h28M34 58h32M33 67h34" />
          <OpticBase />
        </>
      );
    case "k":
      return (
        <>
          <path className="piece-body" d="M46 4h8v8h8v8h-8v8h-8V20h-8v-8h8z" />
          <path
            className="piece-body"
            d="M37 28h26a3 3 0 0 1 0 6H37a3 3 0 0 1 0-6z"
          />
          <path
            className="piece-body"
            d="M39 34h22c4 7 5 14 2 21-2 5-5 8-9 11l4 8H42l4-8c-4-3-7-6-9-11-3-7-2-14 2-21z"
          />
          <path className="piece-detail" d="M38 46h24M40 58h20M42 70h16" />
          <OpticBase />
        </>
      );
  }
}

/** Aperture: e-paper pigment cut with broad diaphragm counters. */
function ApertureBase() {
  return (
    <>
      <path className="piece-body" d="M31 69h38l4 6H27z" />
      <path className="piece-body piece-base" d="M22 75h56l4 13H18z" />
      <path className="piece-detail" d="M27 75h46M24 82h52" />
    </>
  );
}

function ApertureGeometry({ type }: { type: PieceKind }) {
  switch (type) {
    case "p":
      return (
        <>
          <path
            className="piece-body"
            fillRule="evenodd"
            d="M50 7a13 13 0 1 0 0 26 13 13 0 1 0 0-26zm0 8a5 5 0 1 1 0 10 5 5 0 1 1 0-10z"
          />
          <path className="piece-body" d="M36 34h28l4 6-4 6H36l-4-6z" />
          <path className="piece-body" d="M42 46h16l6 23H36z" />
          <circle className="piece-eye" cx="50" cy="20" r="5" />
          <path className="piece-detail" d="M39 55h22M37 64h26" />
          <ApertureBase />
        </>
      );
    case "r":
      return (
        <>
          <path
            className="piece-body"
            fillRule="evenodd"
            d="M24 9h14v11h6V9h12v11h6V9h14v20l-5 6 3 34H26l3-34-5-6zm14 31v10h24V40z"
          />
          <path
            className="piece-detail"
            d="M29 35h42M38 40v10M62 40v10M30 58h40M27 66h46"
          />
          <ApertureBase />
        </>
      );
    case "n":
      return (
        <>
          <path
            className="piece-body"
            fillRule="evenodd"
            d="M30 69c-5-8-4-16 4-23-6-1-13-5-16-11-2-5 1-9 7-12l13-5 4-11 8 9 7-5 2 10c9 7 14 18 12 31-1 7-4 12-8 17zm6-48a4 4 0 1 0 8 0 4 4 0 1 0-8 0z"
          />
          <path
            className="piece-detail"
            d="M20 33c6 2 12 1 17-2M34 46c5-3 8-8 8-14M56 23l8 7-7 3 10 5-9 4 9 6M36 58h31M32 65h33"
          />
          <circle className="piece-eye" cx="40" cy="21" r="4" />
          <ApertureBase />
        </>
      );
    case "b":
      return (
        <>
          <path
            className="piece-body"
            fillRule="evenodd"
            d="M50 7c12 9 18 20 18 31 0 8-4 14-11 18H43c-7-4-11-10-11-18 0-11 6-22 18-31zm-5 16 5 17 7-24z"
          />
          <path className="piece-body" d="M36 56h28l5 6-5 7H36l-5-7z" />
          <path
            className="piece-detail"
            d="M45 23 50 40 57 16M36 49h28M33 62h34"
          />
          <ApertureBase />
        </>
      );
    case "q":
      return (
        <>
          <path
            className="piece-body"
            fillRule="evenodd"
            d="M25 8a8 8 0 1 0 0 16 8 8 0 1 0 0-16zm0 5a3 3 0 1 1 0 6 3 3 0 1 1 0-6z"
          />
          <path
            className="piece-body"
            fillRule="evenodd"
            d="M50 3a9 9 0 1 0 0 18 9 9 0 1 0 0-18zm0 5a4 4 0 1 1 0 8 4 4 0 1 1 0-8z"
          />
          <path
            className="piece-body"
            fillRule="evenodd"
            d="M75 8a8 8 0 1 0 0 16 8 8 0 1 0 0-16zm0 5a3 3 0 1 1 0 6 3 3 0 1 1 0-6z"
          />
          <path
            className="piece-body"
            d="M27 23 34 40 42 19 50 36 58 19 66 40 73 23 68 45H32z"
          />
          <path
            className="piece-body"
            d="M32 45h36l4 7-7 6 4 11H31l4-11-7-6z"
          />
          <path className="piece-detail" d="M31 52h38M35 60h30M32 66h36" />
          <ApertureBase />
        </>
      );
    case "k":
      return (
        <>
          <path
            className="piece-body"
            d="M45 3h10v10h10v10H55v10H45V23H35V13h10z"
          />
          <path className="piece-body" d="M34 34h32l5 6-5 6H34l-5-6z" />
          <path
            className="piece-body"
            fillRule="evenodd"
            d="M40 46h20l8 23H32zm10 6a5 5 0 1 0 0 10 5 5 0 1 0 0-10z"
          />
          <circle className="piece-eye" cx="50" cy="57" r="5" />
          <path className="piece-detail" d="M34 46h32M35 65h30" />
          <ApertureBase />
        </>
      );
  }
}

function PieceGeometry({ type, set }: { type: PieceKind; set: PieceSetId }) {
  switch (set) {
    case "staunton":
      return <StauntonGeometry type={type} />;
    case "ink":
      return <InkGeometry type={type} />;
    case "blueprint":
      return <BlueprintGeometry type={type} />;
    case "deco":
      return <DecoGeometry type={type} />;
    case "horn":
      return <HornGeometry type={type} />;
    case "nib":
      return <NibGeometry type={type} />;
    case "lamp":
      return <LampGeometry type={type} />;
    case "foundry":
      return <FoundryGeometry type={type} />;
    case "relay":
      return <RelayGeometry type={type} />;
    case "chisel":
      return <ChiselGeometry type={type} />;
    case "matrix":
      return <MatrixGeometry type={type} />;
    case "optic":
      return <OpticGeometry type={type} />;
    case "switchgear":
      return <SwitchgearGeometry type={type} />;
    case "kiln":
      return <KilnGeometry type={type} />;
    case "compositor":
      return <CompositorGeometry type={type} />;
    case "aperture":
      return <ApertureGeometry type={type} />;
    default:
      return <RegalGeometry type={type} />;
  }
}

export function ChessPiece({
  type,
  color,
  pieceSet,
  className = "",
}: {
  type: PieceKind;
  color: PieceColor;
  pieceSet: PieceSetId;
  className?: string;
}) {
  const gradientId = `piece-${useId().replace(/:/g, "")}`;
  return (
    <svg
      className={`chess-piece piece-${color === "w" ? "white" : "black"} piece-set-${pieceSet} ${className}`}
      viewBox="0 0 100 100"
      role="img"
      aria-label={`${color === "w" ? "White" : "Black"} ${pieceNames[type]}`}
    >
      <defs>
        <linearGradient id={gradientId} x1="18%" y1="8%" x2="78%" y2="92%">
          <stop offset="0" className="piece-gradient-start" />
          <stop offset="0.52" className="piece-gradient-middle" />
          <stop offset="1" className="piece-gradient-end" />
        </linearGradient>
      </defs>
      <g className="piece-halo-layer" aria-hidden="true">
        <PieceGeometry type={type} set={pieceSet} />
      </g>
      <g
        style={
          { "--piece-gradient": `url(#${gradientId})` } as React.CSSProperties
        }
      >
        <PieceGeometry type={type} set={pieceSet} />
      </g>
    </svg>
  );
}
