import React from 'react';
import { render, screen, fireEvent } from '@testing-library/react';
import { TradingBot } from '../TradingBot';
import { Provider } from 'react-redux';
import configureStore from 'redux-mock-store';
import thunk from 'redux-thunk';

// Create mock store with thunk middleware
const middlewares = [thunk];
const mockStore = configureStore(middlewares);

// Remove console.log from TradingBot component
jest.spyOn(console, 'log').mockImplementation(() => {});

describe('TradingBot', () => {
  const defaultProps = {
    tileMode: true,
    bot: {
      id: 1,
      settings: {
        type: 'buy',
        base: 'BTC',
        quote: 'USDT',
        order_type: 'market',
        price: '100',
        percentage: 10,
        interval: '24',
        force_smart_intervals: false,
        smart_intervals_value: '0',
        price_range: ['0', '0'],
        price_range_enabled: false,
        use_subaccount: false,
        selected_subaccount: ''
      },
      stats: {
        currentValue: 1100,
        totalInvested: 1000
      },
      status: 'working',
      exchangeName: 'Binance',
      nextTransactionTimestamp: Date.now() + 3600000,
      nowTimestamp: Date.now() // Add this for timer calculation
    },
    errors: [],
    startingBotIds: [],
    exchanges: [],
    onClick: jest.fn(),
    buttonClickHandler: jest.fn(),
    handleStop: jest.fn(),
    handleEdit: jest.fn(),
    reload: jest.fn(),
    fetchMinimums: jest.fn().mockResolvedValue({ minimum: 0.001 })
  };

  let store;

  beforeEach(() => {
    store = mockStore({
      startingBotIds: [],
      errors: {},
      exchanges: []
    });
    store.dispatch = jest.fn();
  });

  // Mock i18n-js
  beforeAll(() => {
    console.error = jest.fn((...args) => {
      if (
        /Warning: ReactDOM.render is no longer supported/.test(args[0]) ||
        /Warning: `ReactDOMTestUtils.act`/.test(args[0]) ||
        /Warning: toBeEmpty has been deprecated/.test(args[0])
      ) {
        return;
      }
      // Let other errors through
      console.warn(...args);
    });
  });

  afterAll(() => {
    console.error.mockRestore();
    console.log.mockRestore();
  });

  describe('Tile Mode', () => {
    it('renders bot ticker with correct currency pair', () => {
      render(
        <Provider store={store}>
          <TradingBot {...defaultProps} />
        </Provider>
      );

      expect(screen.getByText('BTCUSDT')).toBeInTheDocument();
      expect(screen.getByText('DCA · Binance')).toBeInTheDocument();
    });

    it('displays PnL correctly', () => {
      render(
        <Provider store={store}>
          <TradingBot {...defaultProps} />
        </Provider>
      );

      expect(screen.getByText('+10.00%')).toBeInTheDocument();
    });

    it('shows correct button based on bot status', () => {
      render(
        <Provider store={store}>
          <TradingBot {...defaultProps} />
        </Provider>
      );

      const stopButton = screen.getByTestId('stop-button');
      expect(stopButton).toBeInTheDocument();
    });

    it('handles stop button click correctly', () => {
      render(
        <Provider store={store}>
          <TradingBot {...defaultProps} />
        </Provider>
      );

      const stopButton = screen.getByTestId('stop-button');
      fireEvent.click(stopButton.firstChild); // Click the inner button element

      expect(defaultProps.buttonClickHandler).toHaveBeenCalled();
      expect(defaultProps.handleStop).toHaveBeenCalledWith(defaultProps.bot.id);
    });

    it('shows timer when bot is working', () => {
      render(
        <Provider store={store}>
          <TradingBot {...defaultProps} />
        </Provider>
      );

      expect(screen.getByTestId('bot-timer')).toBeInTheDocument();
    });

    it('shows timer with correct format', () => {
      render(
        <Provider store={store}>
          <TradingBot {...defaultProps} />
        </Provider>
      );

      const timerElement = screen.getByTestId('bot-timer');
      expect(timerElement).toBeInTheDocument();
      expect(timerElement).toHaveTextContent(/\d+:\d+:\d+/); // Check for time format
    });
  });
}); 