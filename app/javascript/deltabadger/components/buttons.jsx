import React, { useEffect, useState, useRef } from 'react';

export const StartButton = ({onClick}) => (
  <div onClick={onClick} className="btn btn-success">
    <span className="d-none d-sm-inline">Start</span>
    <svg className="btn__svg-icon db-svg-icon db-svg-icon--play" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M8 6.8v10.4a1 1 0 001.5.8l8.2-5.2a1 1 0 000-1.7L9.5 6a1 1 0 00-1.5.8z"/></svg>
  </div>
)
export const StopButton = ({onClick}) => (
  <div onClick={onClick} className="btn btn-outline-primary">
  <span className="d-none d-sm-inline">Pause</span>
  <svg className="btn__svg-icon db-svg-icon db-svg-icon--pause" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M8 19a2 2 0 002-2V7c0-1.1-.9-2-2-2s-2 .9-2 2v10c0 1.1.9 2 2 2zm6-12v10c0 1.1.9 2 2 2s2-.9 2-2V7c0-1.1-.9-2-2-2s-2 .9-2 2z"/></svg>
  </div>
)

export const RemoveButton = ({onClick, disabled}) => {
  const [isOpen, setOpen] = useState(false)
  const node = useRef()

  const handleClickOutside = e => {
    if (node.current && node.current.contains(e.target)) {
      return;
    }
    setOpen(false)
  };

  useEffect(() => {
    document.addEventListener("mousedown", handleClickOutside);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, []);

  return(
    <div className="db-bot__footer">
      <div
        onClick={() => setOpen(true) }
        className={`btn btn-link btn--reset text-secondary ${disabled ? 'disabled' : ''}`}
      >
        <i className="material-icons-round">close</i>
        <span>Delete</span>
      </div>

      { isOpen &&
        <div ref={node} className="db-bot__modal">
          <div className="db-bot__modal__content">
            <p className="">That will remove the bot with all<br/>its historical data. Are you sure?</p>
            <div onClick={() => {setOpen(false)}} className="btn btn-outline-primary mr-2">Cancel</div>
            <div onClick={() => {onClick() && setOpen(false)}} className="btn btn-danger">Remove completely</div>
          </div>
        </div>
      }
    </div>
  )
}

export const CloseButton = ({onClick}) => (
  <div
    onClick={onClick}
    className="btn btn-link btn--reset"
  >
    <i className="material-icons-round">close</i>
    <span>Delete</span>
  </div>
)

export const ExchangeButton = ({ handleClick, exchange }) => (
  <div className={`col-sm-6 col-md-4 db-bot__exchanges__item db-bot__exchanges__item--${exchange.name.toLowerCase()}`} onClick={ () => handleClick(exchange.id) }></div>
)
