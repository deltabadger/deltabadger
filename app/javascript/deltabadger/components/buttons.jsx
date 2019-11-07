import React from 'react';

export const StartButton = ({onClick}) => (
  <div onClick={onClick} className="btn btn-success"><span>Start</span> <i className="material-icons">play_arrow</i></div>
)
export const StopButton = ({onClick}) => (
  <div onClick={onClick} className="btn btn-outline-primary"><span>Pause</span> <i className="material-icons">pause</i></div>
)

export const RemoveButton = ({onClick}) => (
  <div
    onClick={onClick}
    className="btn btn-link btn--reset"
  >
    <i className="material-icons">sync</i>
    <span>Reset</span>
  </div>
)

export const CloseButton = ({onClick}) => (
  <div
    onClick={onClick}
    className="btn btn-link btn--reset"
  >
    <i className="material-icons">close</i>
    <span>Close</span>
  </div>
)

export const ExchangeButton = ({ handleClick, exchange }) => (
  <div className={`col-sm-6 col-md-4 db-bot__exchanges__item db-bot__exchanges__item--${exchange.name.toLowerCase()}`} onClick={ () => handleClick(exchange.id) }></div>
)
