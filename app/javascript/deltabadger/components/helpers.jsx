import React from 'react';
import API from "../lib/API";
import I18n from "i18n-js";

export const removeInvalidApiKeys = (id = null, name = null) => {
  API.removeInvalidApiKeys({ exchangeId: id, exchangeName: name })
}
