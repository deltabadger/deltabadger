const KRAKEN = 'Kraken'
const SYMBOLS_MAP = {
    XDG:'DOGE',
    XBT:'BTC',
    XXBT: 'BTC'
}
export const shouldRename = (name) => name === KRAKEN

export const renameSymbol = (s) => {
    return s in SYMBOLS_MAP ? SYMBOLS_MAP[s] : s
}

export const getSpecialSymbols = (name, isBase) => {
    const shouldRenameSymbols = shouldRename(name)
    const BTC = shouldRenameSymbols ? 'XBT' : 'BTC'

    return isBase ? [BTC, 'ETH'] : ['EUR', 'USD']
}

export const renameCurrency = (currency, exchange) => {
    return shouldRename(exchange) ? renameSymbol(currency) : currency
}