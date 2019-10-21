import React from 'react'
import { BitBayInstructions } from './BitBayInstructions';
import { KrakenInstructions } from './KrakenInstructions';

export const Instructions = ({ exchangeName }) => {
  if (exchangeName == 'BitBay') {
    return <BitBayInstructions />
  } else if (exchangeName == 'Kraken') {
    return <KrakenInstructions />
  } else {
    return "";
  }
}
