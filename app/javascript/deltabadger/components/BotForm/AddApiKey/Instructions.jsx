import React from 'react'
import { BitBayInstructions } from './BitBayInstructions';
import { BitCludeInstructions } from './BitCludeInstructions';
import { KrakenInstructions } from './KrakenInstructions';

export const Instructions = ({ exchangeName }) => {
  if (exchangeName == 'BitBay') {
    return <BitBayInstructions />
  } else if (exchangeName == 'BitClude') {
    return <BitCludeInstructions />
  } else if (exchangeName == 'Kraken') {
    return <KrakenInstructions />
  } else {
    return "";
  }
}
