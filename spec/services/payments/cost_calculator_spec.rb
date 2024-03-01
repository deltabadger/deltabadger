RSpec.describe PaymentsManager::CostCalculator do
  let(:calculator) do
    described_class.new(
      base_price: base_price,
      vat: vat,
      flat_discount: flat_discount,
      discount_percent: discount_percent,
      commission_percent: commission_percent
    )
  end

  let(:vat) { 0 }
  let(:flat_discount) { 0 }
  let(:discount_percent) { 0 }
  let(:commission_percent) { 0 }

  shared_examples 'returns expected values' do |expected|
    methods = %i[
      base_price
      vat
      flat_discount
      discount_percent
      commission_percent
      base_price_with_vat
      total_price
      commission
    ]

    it 'returns values as BigDecimals' do
      methods.each do |method|
        expect(calculator.public_send(method).class).to eq(BigDecimal)
      end
    end

    methods.each do |method|
      it "returns expected #{method}" do
        expect(calculator.public_send(method)).to eq(expected[method])
      end
    end
  end

  shared_examples 'returns crypto_commission' do |crypto_total_price:, expected:|
    it 'returns BigDecimal' do
      expect(calculator.crypto_commission(crypto_total_price: crypto_total_price).class)
        .to eq(BigDecimal)
    end

    it 'returns expected value' do
      expect(calculator.crypto_commission(crypto_total_price: crypto_total_price)).to eq(expected)
    end
  end

  context 'given only base price' do
    let(:base_price) { 20 }

    expected = {
      base_price: BigDecimal('20'),
      vat: BigDecimal('0'),
      flat_discount: BigDecimal('0'),
      discount_percent: BigDecimal('0'),
      commission_percent: BigDecimal('0'),
      base_price_with_vat: BigDecimal('20'),
      total_price: BigDecimal('20'),
      commission: BigDecimal('0')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission', crypto_total_price: 0.001, expected: 0
  end

  context 'given base price and vat' do
    let(:base_price) { 10 }
    let(:vat) { 0.23 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0.23'),
      flat_discount: BigDecimal('0'),
      discount_percent: BigDecimal('0'),
      commission_percent: BigDecimal('0'),
      base_price_with_vat: BigDecimal('12.3'),
      total_price: BigDecimal('12.3'),
      commission: BigDecimal('0')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission', crypto_total_price: 0.001, expected: 0
  end

  context 'given base price and discount' do
    let(:base_price) { 24 }
    let(:discount_percent) { 0.33 }

    expected = {
      base_price: BigDecimal('24'),
      vat: BigDecimal('0'),
      flat_discount: BigDecimal('0'),
      discount_percent: BigDecimal('0.33'),
      commission_percent: BigDecimal('0'),
      base_price_with_vat: BigDecimal('24'),
      total_price: BigDecimal('16.08'),
      commission: BigDecimal('0')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission', crypto_total_price: 0.001, expected: 0
  end

  context 'given base price and flat discount' do
    let(:base_price) { 24 }
    let(:flat_discount) { 5 }

    expected = {
      base_price: BigDecimal('24'),
      vat: BigDecimal('0'),
      flat_discount: BigDecimal('5'),
      discount_percent: BigDecimal('0'),
      commission_percent: BigDecimal('0'),
      base_price_with_vat: BigDecimal('24'),
      total_price: BigDecimal('19'),
      commission: BigDecimal('0')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission', crypto_total_price: 0.001, expected: 0
  end

  context 'given base price, vat and discount' do
    let(:base_price) { 10 }
    let(:vat) { 0.23 }
    let(:discount_percent) { 0.15 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0.23'),
      flat_discount: BigDecimal('0'),
      discount_percent: BigDecimal('0.15'),
      commission_percent: BigDecimal('0'),
      base_price_with_vat: BigDecimal('12.3'),
      total_price: BigDecimal('10.45'),
      commission: BigDecimal('0')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission', crypto_total_price: 0.001, expected: 0
  end

  context 'given base price, vat and flat discount' do
    let(:base_price) { 10 }
    let(:vat) { 0.23 }
    let(:flat_discount) { 3 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0.23'),
      flat_discount: BigDecimal('3'),
      discount_percent: BigDecimal('0'),
      commission_percent: BigDecimal('0'),
      base_price_with_vat: BigDecimal('12.3'),
      total_price: BigDecimal('8.61'),
      commission: BigDecimal('0')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission', crypto_total_price: 0.001, expected: 0
  end

  context 'given base price, vat and flat discount and discount' do
    let(:base_price) { 10 }
    let(:vat) { 0.23 }
    let(:flat_discount) { 3 }
    let(:discount_percent) { 0.1 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0.23'),
      flat_discount: BigDecimal('3'),
      discount_percent: BigDecimal('0.1'),
      commission_percent: BigDecimal('0'),
      base_price_with_vat: BigDecimal('12.3'),
      total_price: BigDecimal('7.74'),
      commission: BigDecimal('0')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission', crypto_total_price: 0.001, expected: 0
  end

  context 'given base price and commission' do
    let(:base_price) { 10 }
    let(:commission_percent) { 0.15 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0'),
      flat_discount: BigDecimal('0'),
      discount_percent: BigDecimal('0'),
      commission_percent: BigDecimal('0.15'),
      base_price_with_vat: BigDecimal('10'),
      total_price: BigDecimal('10'),
      commission: BigDecimal('1.5')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission',
                     crypto_total_price: 0.001,
                     expected: BigDecimal('0.00015')
  end

  context 'given base price, vat and commission' do
    let(:base_price) { 10 }
    let(:vat) { 0.23 }
    let(:commission_percent) { 0.15 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0.23'),
      flat_discount: BigDecimal('0'),
      discount_percent: BigDecimal('0'),
      commission_percent: BigDecimal('0.15'),
      base_price_with_vat: BigDecimal('12.3'),
      total_price: BigDecimal('12.3'),
      commission: BigDecimal('1.5')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission',
                     crypto_total_price: 2 * 0.00123,
                     expected: BigDecimal('0.0003')
  end

  context 'given base price, discount and commission' do
    let(:base_price) { 10 }
    let(:discount_percent) { 0.20 }
    let(:commission_percent) { 0.15 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0'),
      flat_discount: BigDecimal('0'),
      discount_percent: BigDecimal('0.2'),
      commission_percent: BigDecimal('0.15'),
      base_price_with_vat: BigDecimal('10'),
      total_price: BigDecimal('8'),
      commission: BigDecimal('1.5')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission',
                     crypto_total_price: 3 * 0.0008,
                     expected: BigDecimal('0.00045')
  end

  context 'given base price, flat discount, discount and commission' do
    let(:base_price) { 12 }
    let(:flat_discount) { 2 }
    let(:discount_percent) { 0.20 }
    let(:commission_percent) { 0.15 }

    expected = {
      base_price: BigDecimal('12'),
      vat: BigDecimal('0'),
      flat_discount: BigDecimal('2'),
      discount_percent: BigDecimal('0.2'),
      commission_percent: BigDecimal('0.15'),
      base_price_with_vat: BigDecimal('12'),
      total_price: BigDecimal('8'),
      commission: BigDecimal('1.5')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission',
                     crypto_total_price: 3 * 0.0008,
                     expected: BigDecimal('0.00045')
  end

  context 'given base price, discount, vat and commission' do
    let(:base_price) { 10 }
    let(:vat) { 0.23 }
    let(:discount_percent) { 0.20 }
    let(:commission_percent) { 0.15 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0.23'),
      flat_discount: BigDecimal('0'),
      discount_percent: BigDecimal('0.2'),
      commission_percent: BigDecimal('0.15'),
      base_price_with_vat: BigDecimal('12.3'),
      total_price: BigDecimal('9.84'),
      commission: BigDecimal('1.5')
    }

    include_examples 'returns expected values', expected
    include_examples 'returns crypto_commission',
                     crypto_total_price: 4 * 0.000984,
                     expected: BigDecimal('0.0006')
  end
end
