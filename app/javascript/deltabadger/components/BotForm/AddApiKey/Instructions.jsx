import React from 'react'
import { BinanceInstructions } from './BinanceInstructions';
import { BitBayInstructions } from './BitBayInstructions';
import { KrakenInstructions } from './KrakenInstructions';

export const Instructions = ({ exchangeName }) => {
  switch (exchangeName.toLowerCase()) {
    case 'binance': return <BinanceInstructions binanceName={exchangeName}/>;
    case 'binance.us': return <BinanceInstructions binanceName={exchangeName}/>;
    case 'bitbay': return <BitBayInstructions />;
    case 'kraken': return <KrakenInstructions />;
    default: return '';
  }
}
