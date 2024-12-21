import React from 'react';
import { render } from '@testing-library/react';
import { ProgressBar } from '../ProgressBar';

// Suppress specific React warnings
const originalError = console.error;
beforeAll(() => {
  console.error = (...args) => {
    if (
      /Warning: ReactDOM.render is no longer supported/.test(args[0]) ||
      /Warning: `ReactDOMTestUtils.act`/.test(args[0])
    ) {
      return;
    }
    originalError.call(console, ...args);
  };
});

afterAll(() => {
  console.error = originalError;
});

describe('ProgressBar', () => {
  const defaultProps = {
    bot: {
      settings: { type: 'buy' },
      status: 'working',
      nextTransactionTimestamp: Date.now() + 3600000,
      transactions: [{ created_at_timestamp: Date.now() - 3600000 }]
    }
  };

  it('renders with correct color class based on bot type', () => {
    const { container } = render(<ProgressBar {...defaultProps} />);
    expect(container.querySelector('.bg-success')).toBeInTheDocument();
  });

  it('shows zero progress when bot is not working', () => {
    const props = {
      bot: {
        ...defaultProps.bot,
        status: 'stopped'
      }
    };

    const { container } = render(<ProgressBar {...props} />);
    expect(container.querySelector('.progress-bar').style.width).toBe('0%');
  });
}); 