import { useId } from "react";

export type PieceSetId = "regal" | "staunton" | "ink" | "blueprint" | "deco";
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
