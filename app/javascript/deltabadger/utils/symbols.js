const KRAKEN = "Kraken"
export const shouldRename = (name) => name === KRAKEN

export const renameSymbol = (s) => {
    return s.replace(/(^X|^Z)([A-Z]{3})/, "$2")
        .replace(/^XBT/, "BTC")
}