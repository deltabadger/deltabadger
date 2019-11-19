import React from 'react'
import { createStore, applyMiddleware } from 'redux';
import thunk from 'redux-thunk';
import { createLogger } from 'redux-logger';

export const configureStore = (reducer) => {
  const middlewares = [
    thunk,
  ];
  if (process.env.NODE_ENV !== 'production') {
    middlewares.push(createLogger());
  }

  return createStore(
    reducer,
    undefined,
    applyMiddleware(...middlewares)
  );
};
