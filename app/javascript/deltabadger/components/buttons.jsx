import React, { useEffect, useState, useRef } from 'react';
import {formatDuration} from "../utils/time";
import moment from "moment";

export const startButtonType = {
    CHANGED_MISSED: "changedMissed",
    CHANGED_ON_SCHEDULE: "changedOnSchedule",
    MISSED: "missed",
    ON_SCHEDULE: "onSchedule",
    FAILED: "failed"
}
let timeout;

export const StartButton = ({settings, getRestartType, onClickReset}) => {
  const [isOpen, setOpen] = useState(false)
  const [getType, setType] = useState(startButtonType.ON_SCHEDULE)
  const [timeToNextTransaction, setTimeToNextTransaction] = useState("")
  const [missedAmount, setMissedAmount] = useState(0.0)
  const node = useRef()

  const handleClickOutside = e => {
    if (node.current && node.current.contains(e.target)) {
      return;
    }
    setOpen(false)
    clearTimeout(timeout)
  };

  const SmarterRestartButtons = () => {
    return (
    <div>
      { getType === startButtonType.CHANGED_ON_SCHEDULE &&
        <div>
          <p className="">While the bot was paused, you missed part of the schedule. You have
            still <b>{timeToNextTransaction}</b> to the next order. The new schedule will start after
            the current counting is finished.</p>
          <div className="db-bot__modal__btn-group">
            <div onClick={() => {
              onClickReset() && setOpen(false)
            }} className="btn btn-outline-primary">Start, from now!
            </div>
            <div onClick={() => {
              onClickReset(true) && setOpen(false)
            }} className="btn btn-success">Start since next transaction!
            </div>
          </div>
          </div>
      }
      { getType === startButtonType.CHANGED_MISSED &&
        <div>
          <p className="">While the bot was paused, you missed part of the schedule. Do you want invest
            missed <b>{missedAmount.toFixed(3)} {settings.quote}</b> and stick to the schedule?
            The new schedule will start after the current counting is finished.</p>
          <div className="db-bot__modal__btn-group">
            <div onClick={() => {
              onClickReset() && setOpen(false)
            }} className="btn btn-outline-primary">Start, without buying!
            </div>
            <div onClick={() => {
              onClickReset(false, missedAmount) && setOpen(false)
            }} className="btn btn-success">Buy, then start with new parameters.
            </div>
          </div>
      </div>
      }
      { getType === startButtonType.MISSED &&
        <div>
          <p className="">While the bot was paused, you missed part of the schedule. Do you want invest
            missed <b>{missedAmount.toFixed(3)} {settings.quote}</b> and continue the original counting?</p>
          <div className="db-bot__modal__btn-group">
            <div onClick={() => {
              onClickReset() && setOpen(false)
            }} className="btn btn-outline-primary">No, skip it and start new
            </div>
            <div onClick={() => {
              onClickReset(false, missedAmount) && setOpen(false)
            }} className="btn btn-success">Yes, stick to the schedule
            </div>
          </div>
          </div>
      }
      { getType === startButtonType.ON_SCHEDULE &&
        <div>
          <p className="">While the bot was paused, you missed part of the schedule. You have
            still <b>{timeToNextTransaction}</b> to the next order.</p>
          <div className="db-bot__modal__btn-group">
            <div onClick={() => {
              onClickReset() && setOpen(false)
            }} className="btn btn-primary">No, start again from now
            </div>
            <div onClick={() => {
              onClickReset(true) && setOpen(false)
            }} className="btn btn-success">Yes, follow the schedule
            </div>
          </div>
        </div>
      }
    </div>
    )
  }

  const handleOnClick = () => {
    getRestartType().then((data) => {
      switch (data.restartType) {
        case startButtonType.FAILED:
          onClickReset()
          return
        case startButtonType.ON_SCHEDULE:
        case startButtonType.CHANGED_ON_SCHEDULE:
          setTimeToNextTransaction(formatDuration(moment.duration(data.timeToNextTransaction, 'seconds')) )
          break
        case startButtonType.MISSED:
        case startButtonType.CHANGED_MISSED:
          setMissedAmount(data.missedAmount)
          break
      }
      setType(data.restartType)
      setOpen(true)
      timeout = setTimeout(() => setOpen(false), data.timeout)
    })
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
         onClick={handleOnClick}
         className="btn btn-success">
        <span className="d-none d-sm-inline">Start</span>
        <svg className="btn__svg-icon db-svg-icon db-svg-icon--play" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M8 6.8v10.4a1 1 0 001.5.8l8.2-5.2a1 1 0 000-1.7L9.5 6a1 1 0 00-1.5.8z"/></svg>
     </div>
        { isOpen &&
        <div ref={node} className="db-bot__modal">
          <div className="db-bot__modal__content">
            <SmarterRestartButtons />
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
            <div className="db-bot__modal__btn-group">
              <div onClick={() => {setOpen(false)}} className="btn btn-outline-primary">Cancel</div>
              <div onClick={() => {onClick() && setOpen(false)}} className="btn btn-danger">Remove completely</div>
            </div>
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
