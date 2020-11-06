import React, { useEffect, useState, useRef } from 'react';

export const startButtonType = {
    CHANGED: "changed",
    MISSED: "missed",
    ON_SCHEDULE: "onSchedule"
}

export const StartButton = ({onClickReset, onClickContinue, type}) => {
  const [isOpen, setOpen] = useState(false)
  const node = useRef()

  const handleClickOutside = e => {
    if (node.current && node.current.contains(e.target)) {
      return;
    }
    setOpen(false)
  };

  const SmarterRestartButtons = ({type}) => {
      switch (type) {
          case startButtonType.CHANGED:
              return(
                  <div>
                      <p className="">The schedule has changed. The bot will start<br/>buying according to the new
                          schedule right away.</p>
                      <div onClick={() => {
                          setOpen(false)
                      }} className="btn btn-outline-primary mr-2">Cancel
                      </div>
                      <div onClick={() => {
                          onClickReset() && setOpen(false)
                      }} className="btn btn-success">Yes, let's go!
                      </div>
                  </div>
              )
          case startButtonType.MISSED:
              return (
                  <div>
                      <p className="">While the bot was paused, you missed part of the schedule.<br/>Do you want invest
                          missed 150USD and stick to the schedule?</p>
                      <div onClick={() => {
                          onClickReset() && setOpen(false)
                      }} className="btn btn-primary mr-2">No, start again from now
                      </div>
                      <div onClick={() => {
                          onClickContinue() && setOpen(false)
                      }} className="btn btn-success">Yes, follow the schedule
                      </div>
                  </div>
              )
          case startButtonType.ON_SCHEDULE:
              return (
                  <div>
                      <p className="">While the bot was paused, you missed part of the schedule.<br/>You have still 1d 3h
                          12m 44s to the next order.</p>
                      <div onClick={() => {
                          onClickReset() && setOpen(false)
                      }} className="btn btn-primary mr-2">No, start again from now
                      </div>
                      <div onClick={() => {
                          onClickContinue() && setOpen(false)
                      }} className="btn btn-success">Yes, follow the schedule
                      </div>
                  </div>
              )
        }
  }

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
         className="btn btn-success">
        <span className="d-none d-sm-inline">Start</span>
        <svg className="btn__svg-icon db-svg-icon db-svg-icon--play" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M8 6.8v10.4a1 1 0 001.5.8l8.2-5.2a1 1 0 000-1.7L9.5 6a1 1 0 00-1.5.8z"/></svg>
     </div>
        { isOpen &&
        <div ref={node} className="db-bot__modal">
          <div className="db-bot__modal__content">
            <SmarterRestartButtons type={type} />
          </div>
        </div>
        }
     </div>
  )
}

export const StartingButton = () => (
  <div className="btn btn-success disabled"><span className="d-none d-sm-inline">Starting</span> <i className="material-icons-round">play_arrow</i></div>
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

export const ExchangeButton = ({ handleClick, exchange }) => {
  return (
    <div
      className={`col-sm-6 col-md-4 db-bot__exchanges__item db-bot__exchanges__item--${exchange.name.toLowerCase()}`}
      onClick={() => handleClick(exchange.id)}
    >
      { exchange.name }
    </div>
  );
}
