import React from 'react';
import API from "../lib/API";

export const removeInvalidApiKeys = (id = null, name = null) => {
  API.removeInvalidApiKeys({ exchangeId: id, exchangeName: name })
}

export const splitTranslation = (s) => {
  return s.split(/<split>.*?<\/split>/)
}

export const toFixedWithoutZeros = (x) => {
  if(parseFloat(x) >= 1.0 || parseFloat(x) <= -1.0){
    return parseFloat(x).toFixed(2);
  }

  return parseFloat(x).toFixed(8).replace(/\.?0*$/,'');
}
