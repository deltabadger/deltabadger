const KRAKEN = "Kraken"
export const shouldRename = (name) => name === KRAKEN

export const renameSymbol = (s) => {
    return s.replace(/(^X|^Z)([A-Z]{3}[A-Z]*)/, "$2")
        .replace(/^XBT/, "BTC")
}

export const getSpecialSymbols = (name, isBase) => {
    const shouldRenameSymbols = shouldRename(name)
    const BTC = shouldRenameSymbols ? "XXBT" : "BTC"
    const ETH = shouldRenameSymbols ? "XETH" : "ETH"
    const EUR = shouldRenameSymbols ? "ZEUR" : "EUR"
    const USD = shouldRenameSymbols ? "ZUSD" : "USD"

    return isBase ? [BTC, ETH] : [EUR, USD]
}