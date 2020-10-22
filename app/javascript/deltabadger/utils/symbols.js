const KRAKEN = 'Kraken'
export const shouldRename = (name) => name === KRAKEN

export const renameSymbol = (s) => {
    return s.replace(/^XBT/, 'BTC')
}

export const getSpecialSymbols = (name, isBase) => {
    const shouldRenameSymbols = shouldRename(name)
    const BTC = shouldRenameSymbols ? 'XBT' : 'BTC'
    const ETH = 'ETH'
    const EUR = 'EUR'
    const USD = 'USD'

    return isBase ? [BTC, ETH] : [EUR, USD]
}