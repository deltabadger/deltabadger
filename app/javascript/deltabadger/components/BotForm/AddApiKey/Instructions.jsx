import React from 'react'
import { BinanceInstructions } from './BinanceInstructions';
import { BitBayInstructions } from './BitBayInstructions';
import { KrakenInstructions } from './KrakenInstructions';
import { CoinbaseProInstructions } from './CoinbaseProInstructions';
import { GeminiInstructions } from "./GeminiInstructions";
import { FtxInstructions } from "./FtxInstructions";
import { BitsoInstructions } from "./BitsoInstructions";

export const Instructions = ({ exchangeName }) => {
  switch (exchangeName.toLowerCase()) {
    case 'binance': return <BinanceInstructions binanceName={exchangeName}/>;
    case 'binance.us': return <BinanceInstructions binanceName={exchangeName}/>;
    case 'bitbay': return <BitBayInstructions />;
    case 'kraken': return <KrakenInstructions />;
    case 'coinbase pro': return <CoinbaseProInstructions />;
    case 'gemini': return <GeminiInstructions />;
    case 'ftx': return <FtxInstructions />;
    case 'bitso': return <BitsoInstructions />;
    default: return '';
  }
}
