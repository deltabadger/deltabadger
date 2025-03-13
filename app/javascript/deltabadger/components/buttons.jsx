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
const NOT_RELEVANT_BOTS = ["FTX", "FTX.US", "Coinbase Pro"];

export const StartButton = ({settings, getRestartType, onClickReset, setShowInfo, exchangeName, newSettings}) => {
  const [isOpen, setOpen] = useState(false)
  const [getType, setType] = useState(startButtonType.ON_SCHEDULE)
  const [timeToNextTransaction, setTimeToNextTransaction] = useState("")
  const [missedAmount, setMissedAmount] = useState(0.0)
  const [showStart, setShowStart] = useState(!NOT_RELEVANT_BOTS.includes(exchangeName))
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
            }} className="button button--primary button--outline">{I18n.t('bots.buttons.start.changed_on_schedule.skip')}
            </div>
            <div onClick={() => {
              _handleSubmit(true)
            }} className="button button--success">{I18n.t('bots.buttons.start.changed_on_schedule.continue')}
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
            }} className="button button--primary button--outline">{I18n.t('bots.buttons.start.changed_missed.skip')}
            </div>
            <div onClick={() => {
              _handleSubmit(false, missedAmount)
            }} className="button button--success">{I18n.t('bots.buttons.start.changed_missed.continue')}
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
            }} className="button button--primary button--outline">{I18n.t('bots.buttons.start.missed.skip')}
            </div>
            <div onClick={() => {
              _handleSubmit(false, missedAmount)
            }} className="button button--success">{I18n.t('bots.buttons.start.missed.continue')}
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
            }} className="button button--primary button--outline">{I18n.t('bots.buttons.start.on_schedule.skip')}
            </div>
            <div onClick={() => {
              _handleSubmit(true)
            }} className="button button--success">{I18n.t('bots.buttons.start.on_schedule.continue')}
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
     { showStart &&
     <div
         onClick={_handleRestarts}
         className="button button--success">
        <div className="animicon animicon--start">
          <div className="animicon__a"></div>
          <div className="animicon__b"></div>
        </div>
     </div>
     }
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
  <div className="button button--success disabled">
    <span>{I18n.t('bots.starting')}</span>
  </div>
)
export const PendingButton = () => (
  <div className="button button--success disabled">
    <span>{I18n.t('bots.buttons.pending.text')}</span>
  </div>
)
export const StopButton = ({ onClick }) => (
  <div 
    onClick={onClick} 
    className="button button--primary button--outline"
    data-testid="stop-button"
    role="button"
    aria-label="stop"
  >
    <div className="animicon animicon--stop">
      <div className="animicon__a"></div>
      <div className="animicon__b"></div>
    </div>
  </div>
);

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
        className={`button button--link button--icon-and-text ${disabled ? 'button--disabled' : ''}`}
      >
        <i className="material-icons">close</i>
        <span>{I18n.t('bots.buttons.delete.text')}</span>
      </div>

      { isOpen &&
        <div ref={node} className="db-bot__modal">
          <div className="db-bot__modal__content">
            <RawHTML tag="p">{I18n.t('bots.buttons.delete.warning_html')}</RawHTML>
            <div className="db-bot__modal__btn-group">
              <div onClick={() => {setOpen(false)}} className="button button--primary button--outline">{I18n.t('bots.buttons.delete.cancel')}</div>
              <div onClick={() => {onClick() && setOpen(false)}} className="button button--danger">{I18n.t('bots.buttons.delete.ok')}</div>
            </div>
          </div>
        </div>
      }
    </div>
  )
}

export const ExchangeButton = ({ handleClick, exchange, type }) => {
  const withdrawalEnabled = () => ['kraken', 'ftx', 'ftx.us'];
  const webhookEnabled = () => ['kraken'];

  const exchangeClass = () => {
    if (type === 'trading') {
      return exchange.name.toLowerCase().replace('.', '-');
    }

    if (type === 'withdrawal') {
      return withdrawalEnabled().includes(exchange.name.toLowerCase()) ? exchange.name.toLowerCase() : 'unavailable';
    }

    if (type === 'webhook') {
      return webhookEnabled().includes(exchange.name.toLowerCase()) ? exchange.name.toLowerCase() : 'unavailable';
    }

    return 'unavailable';
  };

  return (
    <div
      className={`db-bot__exchanges__item db-bot__exchanges__item--${exchangeClass()}`}
      onClick={() => handleClick(exchange.id, exchange.name)}
    >
      <div>{exchange.name}</div>
      <div>{exchange.maker_fee}%</div>
      <div>{exchange.taker_fee}%</div>
      <div>{exchange.withdrawal_fee}</div>
    </div>
  );
};

