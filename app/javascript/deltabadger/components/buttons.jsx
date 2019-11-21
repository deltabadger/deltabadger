import React, { useEffect, useState, useRef } from 'react';

export const StartButton = ({onClick}) => (
  <div onClick={onClick} className="btn btn-success"><span>Start</span> <i className="material-icons">play_arrow</i></div>
)
export const StopButton = ({onClick}) => (
  <div onClick={onClick} className="btn btn-outline-primary"><span>Pause</span> <i className="material-icons">pause</i></div>
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
    <div>
      <div
        onClick={() => setOpen(true) }
        className={`btn btn-link btn--reset ${disabled ? 'disabled' : ''}`}
      >
        <i className="material-icons">sync</i>
        <span>Reset</span>
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
    <i className="material-icons">close</i>
    <span>Close</span>
  </div>
)

export const ExchangeButton = ({ handleClick, exchange }) => (
  <div className={`col-sm-6 col-md-4 db-bot__exchanges__item db-bot__exchanges__item--${exchange.name.toLowerCase()}`} onClick={ () => handleClick(exchange.id) }></div>
)
