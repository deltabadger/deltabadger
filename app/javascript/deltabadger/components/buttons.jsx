import React, { useEffect, useState, useRef } from 'react';
import I18n from 'i18n-js'
import { RawHTML } from './RawHtml'
import {formatDurationRestart} from "../utils/time";
import {renameCurrency} from "../utils/symbols";
import moment from "moment";

export const startButtonType = {
    CHANGED_MISSED: "changedMissed",
    CHANGED_ON_SCHEDULE: "changedOnSchedule",
    MISSED: "missed",
    ON_SCHEDULE: "onSchedule",
    FAILED: "failed"
}
let timeout;

export const StartButton = ({settings, getRestartType, onClickReset, setShowInfo, exchangeName, newSettings}) => {
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

  const _handleSubmit = (continueSchedule = false, fixing_price = null) => {
    if(!isOpen) {
      return;
    }

    setOpen(false)
    onClickReset(continueSchedule, fixing_price)
  }

  const cleverToFixed = (amount) => {
    if (amount >= 1) {
      return parseFloat(amount).toFixed(2);
    }

    return parseFloat(parseFloat(amount).toFixed(8));
  }

  const SmarterRestartButtons = () => {
    return (
    <div>
      { getType === startButtonType.CHANGED_ON_SCHEDULE &&
        <div>
          <RawHTML tag="p">{I18n.t('bots.buttons.start.changed_on_schedule.info_html', { time: timeToNextTransaction })}</RawHTML>
          <div className="db-bot__modal__btn-group">
            <div onClick={() => {
              _handleSubmit()
            }} className="btn btn-outline-primary">{I18n.t('bots.buttons.start.changed_on_schedule.skip')}
            </div>
            <div onClick={() => {
              _handleSubmit(true)
            }} className="btn btn-success">{I18n.t('bots.buttons.start.changed_on_schedule.continue')}
            </div>
          </div>
          </div>
      }
      { getType === startButtonType.CHANGED_MISSED &&
        <div>
          <RawHTML tag="p">{I18n.t('bots.buttons.start.changed_missed.info_html', { amount: cleverToFixed(missedAmount), quote: settings.quote })}</RawHTML>
          <div className="db-bot__modal__btn-group">
            <div onClick={() => {
              _handleSubmit()
            }} className="btn btn-outline-primary">{I18n.t('bots.buttons.start.changed_missed.skip')}
            </div>
            <div onClick={() => {
              _handleSubmit(false, missedAmount)
            }} className="btn btn-success">{I18n.t('bots.buttons.start.changed_missed.continue')}
            </div>
          </div>
      </div>
      }
      { getType === startButtonType.MISSED &&
        <div>
          <RawHTML tag="p">{I18n.t('bots.buttons.start.missed.info_html', { amount: cleverToFixed(missedAmount), quote: settings.quote })}</RawHTML>
          <div className="db-bot__modal__btn-group">
            <div onClick={() => {
              _handleSubmit()
            }} className="btn btn-outline-primary">{I18n.t('bots.buttons.start.missed.skip')}
            </div>
            <div onClick={() => {
              _handleSubmit(false, missedAmount)
            }} className="btn btn-success">{I18n.t('bots.buttons.start.missed.continue')}
            </div>
          </div>
          </div>
      }
      { getType === startButtonType.ON_SCHEDULE &&
        <div>
          <RawHTML tag="p">{I18n.t('bots.buttons.start.on_schedule.info_html', { time: timeToNextTransaction })}</RawHTML>
          <div className="db-bot__modal__btn-group">
            <div onClick={() => {
              _handleSubmit()
            }} className="btn btn-outline-primary">{I18n.t('bots.buttons.start.on_schedule.skip')}
            </div>
            <div onClick={() => {
              _handleSubmit(true)
            }} className="btn btn-success">{I18n.t('bots.buttons.start.on_schedule.continue')}
            </div>
          </div>
        </div>
      }
    </div>
    )
  }

  const _handleRestarts = () => {
    getRestartType().then((data) => {
      switch (data.restartType) {
        case startButtonType.FAILED:
          onClickReset()
          return
        case startButtonType.ON_SCHEDULE:
        case startButtonType.CHANGED_ON_SCHEDULE:
          setTimeToNextTransaction(formatDurationRestart(moment.duration(data.timeToNextTransaction, 'seconds')) )
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
         onClick={_handleRestarts}
         className="btn btn-success">
        <span className="d-none d-sm-inline">Start</span>
        <svg className="btn__svg-icon db-svg-icon db-svg-icon--play" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
          <path d="M8 6.8v10.4a1 1 0 001.5.8l8.2-5.2a1 1 0 000-1.7L9.5 6a1 1 0 00-1.5.8z"/>
        </svg>
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
  <div className="btn btn-success disabled">
    <span className="d-none d-sm-inline">{I18n.t('bots.starting')}</span>
    <i className="material-icons">play_arrow</i>
  </div>
)
export const PendingButton = () => (
  <div className="btn btn-success disabled">
    <span className="d-none d-sm-inline">{I18n.t('bots.buttons.pending.text')}</span>
    <i className="material-icons">play_arrow</i>
  </div>
)
export const StopButton = ({onClick}) => (
  <div onClick={onClick} className="btn btn-outline-primary">
    <span className="d-none d-sm-inline">{I18n.t('bots.stop')}</span>
    <svg className="btn__svg-icon db-svg-icon db-svg-icon--pause" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
      <path d="M8 19a2 2 0 002-2V7c0-1.1-.9-2-2-2s-2 .9-2 2v10c0 1.1.9 2 2 2zm6-12v10c0 1.1.9 2 2 2s2-.9 2-2V7c0-1.1-.9-2-2-2s-2 .9-2 2z"/>
    </svg>
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
    <div>
      <div
        onClick={() => setOpen(true) }
        className={`btn btn-link btn--reset text-secondary ${disabled ? 'disabled' : ''}`}
      >
        <i className="material-icons">close</i>
        <span>{I18n.t('bots.buttons.delete.text')}</span>
      </div>

      { isOpen &&
        <div ref={node} className="db-bot__modal">
          <div className="db-bot__modal__content">
            <RawHTML tag="p">{I18n.t('bots.buttons.delete.warning_html')}</RawHTML>
            <div className="db-bot__modal__btn-group">
              <div onClick={() => {setOpen(false)}} className="btn btn-outline-primary">{I18n.t('bots.buttons.delete.cancel')}</div>
              <div onClick={() => {onClick() && setOpen(false)}} className="btn btn-danger">{I18n.t('bots.buttons.delete.ok')}</div>
            </div>
          </div>
        </div>
      }
    </div>
  )
}

export const ExchangeButton = ({ handleClick, exchange, type }) => {
  const withdrawalEnabled = () => {
    return ['kraken', 'ftx', 'ftx.us']
  }

  const exchangeClass = () => {
    if (type === 'trading') {
      return exchange.name.toLowerCase().replace('.', '-')
    }

    return withdrawalEnabled().includes(exchange.name.toLowerCase()) ? exchange.name.toLowerCase() : 'unavailable'
  }

  return (
    <div
      className={`col-sm-6 col-md-4 db-bot__exchanges__item db-bot__exchanges__item--${exchangeClass()}`}
      onClick={() => handleClick(exchange.id, exchange.name)}
    >
      { exchange.name }
    </div>
  );
}
