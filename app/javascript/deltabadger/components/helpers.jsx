import React from 'react';
import API from "../lib/API";

export const removeInvalidApiKeys = (id = null, name = null) => {
  API.removeInvalidApiKeys({ exchangeId: id, exchangeName: name })
}

export const splitTranslation = (s) => {
  return s.split(/<split>.*?<\/split>/)
}
