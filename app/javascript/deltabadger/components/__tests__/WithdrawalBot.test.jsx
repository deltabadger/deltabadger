import React from 'react';
import { render, screen, fireEvent } from '@testing-library/react';
import { WithdrawalBot } from '../WithdrawalBot';
import { Provider } from 'react-redux';
import configureStore from 'redux-mock-store';
import thunk from 'redux-thunk';
import I18n from 'i18n-js';

// Create mock store with thunk middleware
const middlewares = [thunk];
const mockStore = configureStore(middlewares);

jest.mock('i18n-js', () => ({
  t: (key) => {
    const translations = {
      'bot.withdrawal': 'Withdrawal',
      'bot.withdrawal_percentage': 'Withdrawal progress',
      'bot.buttons.pending.info_html': 'Pending'
      // Add other translations as needed
    };
    return translations[key] || `[missing "${key}" translation]`;
  }
}));

describe('WithdrawalBot', () => {
  const defaultProps = {
    tileMode: true,
    bot: {
      id: 1,
      settings: {
        currency: 'BTC',
        threshold_enabled: true,
        interval_enabled: true,
        threshold: '0.1',
        interval: '24'
      },
      stats: {
        totalWithdrawn: 1.5
      },
      status: 'working',
      exchangeName: 'Binance',
      nextTransactionTimestamp: Date.now() + 3600000
    },
    errors: [],
    startingBotIds: [],
    exchanges: [],
    onClick: jest.fn(),
    buttonClickHandler: jest.fn(),
    handleStop: jest.fn(),
    handleEdit: jest.fn(),
    reload: jest.fn(),
    getMinimums: jest.fn().mockResolvedValue({ minimum: 0.001 })
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
        /Warning: `ReactDOMTestUtils.act`/.test(args[0])
      ) {
        return;
      }
      // Let other errors through
      console.warn(...args);
    });
  });

  afterAll(() => {
    console.error.mockRestore();
  });

  describe('Tile Mode', () => {
    it('renders bot ticker with correct currency', () => {
      render(
        <Provider store={store}>
          <WithdrawalBot {...defaultProps} />
        </Provider>
      );

      expect(screen.getByText('BTC')).toBeInTheDocument();
      expect(screen.getByText('Withdrawal · Binance')).toBeInTheDocument();
    });

    it('displays total withdrawn amount correctly', () => {
      render(
        <Provider store={store}>
          <WithdrawalBot {...defaultProps} />
        </Provider>
      );

      expect(screen.getByText('1.5 BTC')).toBeInTheDocument();
    });

    it('shows correct button based on bot status', () => {
      render(
        <Provider store={store}>
          <WithdrawalBot {...defaultProps} />
        </Provider>
      );

      const stopButton = screen.getByRole('button', { name: /stop/i });
      expect(stopButton).toBeInTheDocument();
    });

    it('handles stop button click correctly', () => {
      render(
        <Provider store={store}>
          <WithdrawalBot {...defaultProps} />
        </Provider>
      );

      const stopButton = screen.getByTestId('stop-button');
      fireEvent.click(stopButton);

      expect(defaultProps.buttonClickHandler).toHaveBeenCalled();
      expect(defaultProps.handleStop).toHaveBeenCalledWith(defaultProps.bot.id);
    });

    it('shows timer when bot is working with interval enabled', () => {
      render(
        <Provider store={store}>
          <WithdrawalBot {...defaultProps} />
        </Provider>
      );

      expect(screen.getByTestId('bot-timer')).toBeInTheDocument();
    });

    it('shows percentage progress when interval is disabled', () => {
      const props = {
        ...defaultProps,
        bot: {
          ...defaultProps.bot,
          settings: {
            ...defaultProps.bot.settings,
            interval_enabled: false
          }
        }
      };

      render(
        <Provider store={store}>
          <WithdrawalBot {...props} />
        </Provider>
      );

      expect(screen.getByText('Withdrawal progress')).toBeInTheDocument();
    });
  });
}); 