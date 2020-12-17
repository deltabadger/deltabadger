import React from 'react'
import { BinanceInstructions } from './BinanceInstructions';
import { BinanceUSInstructions } from './BinanceUSInstructions';
import { BitBayInstructions } from './BitBayInstructions';
import { KrakenInstructions } from './KrakenInstructions';

export const Instructions = ({ exchangeName }) => {
  switch (exchangeName.toLowerCase()) {
    case 'binance': return <BinanceInstructions />;
    case 'binanceus': return <BinanceUSInstructions />;
    case 'bitbay': return <BitBayInstructions />;
    case 'kraken': return <KrakenInstructions />;
    default: return '';
  }
}
