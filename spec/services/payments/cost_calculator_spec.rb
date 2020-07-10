RSpec.describe Payments::CostCalculator do
  let(:calculator) do
    described_class.new(
      base_price: base_price,
      vat: vat,
      discount_percent: discount_percent,
      commission_percent: commission_percent
    )
  end

  let(:vat) { 0 }
  let(:discount_percent) { 0 }
  let(:commission_percent) { 0 }

  shared_examples 'returns expected values' do |expected|
    methods = %i[base_price vat discount_percent commission_percent price_with_vat total_price commission]

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

  context 'given only base price' do
    let (:base_price) { 20 }

    expected = {
      base_price: BigDecimal('20'),
      vat: BigDecimal('0'),
      discount_percent: BigDecimal('0'),
      commission_percent: BigDecimal('0'),
      price_with_vat: BigDecimal('20'),
      total_price: BigDecimal('20'),
      commission: BigDecimal('0')
    }

    include_examples 'returns expected values', expected
  end

  context 'given base price and vat' do
    let (:base_price) { 10 }
    let (:vat) { 0.23 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0.23'),
      discount_percent: BigDecimal('0'),
      commission_percent: BigDecimal('0'),
      price_with_vat: BigDecimal('12.3'),
      total_price: BigDecimal('12.3'),
      commission: BigDecimal('0')
    }

    include_examples 'returns expected values', expected
  end

  context 'given base price and discount' do
    let (:base_price) { 24 }
    let (:discount_percent) { 0.33 }

    expected = {
      base_price: BigDecimal('24'),
      vat: BigDecimal('0'),
      discount_percent: BigDecimal('0.33'),
      commission_percent: BigDecimal('0'),
      price_with_vat: BigDecimal('24'),
      total_price: BigDecimal('16.08'),
      commission: BigDecimal('0')
    }

    include_examples 'returns expected values', expected
  end

  context 'given base price, vat and discount' do
    let (:base_price) { 10 }
    let (:vat) { 0.23 }
    let (:discount_percent) { 0.15 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0.23'),
      discount_percent: BigDecimal('0.15'),
      commission_percent: BigDecimal('0'),
      price_with_vat: BigDecimal('12.3'),
      total_price: BigDecimal('10.45'),
      commission: BigDecimal('0')
    }

    include_examples 'returns expected values', expected
  end

  context 'given base price and commission' do
    let (:base_price) { 10 }
    let (:commission_percent) { 0.15 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0'),
      discount_percent: BigDecimal('0'),
      commission_percent: BigDecimal('0.15'),
      price_with_vat: BigDecimal('10'),
      total_price: BigDecimal('10'),
      commission: BigDecimal('1.5')
    }

    include_examples 'returns expected values', expected
  end

  context 'given base price, vat and commission' do
    let (:base_price) { 10 }
    let (:vat) { 0.23 }
    let (:commission_percent) { 0.15 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0.23'),
      discount_percent: BigDecimal('0'),
      commission_percent: BigDecimal('0.15'),
      price_with_vat: BigDecimal('12.3'),
      total_price: BigDecimal('12.3'),
      commission: BigDecimal('1.5')
    }

    include_examples 'returns expected values', expected
  end

  context 'given base price, discount and commission' do
    let (:base_price) { 10 }
    let (:discount_percent) { 0.20 }
    let (:commission_percent) { 0.15 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0'),
      discount_percent: BigDecimal('0.2'),
      commission_percent: BigDecimal('0.15'),
      price_with_vat: BigDecimal('10'),
      total_price: BigDecimal('8'),
      commission: BigDecimal('1.5')
    }

    include_examples 'returns expected values', expected
  end

  context 'given base price, discount, vat and commission' do
    let (:base_price) { 10 }
    let (:vat) { 0.23 }
    let (:discount_percent) { 0.20 }
    let (:commission_percent) { 0.15 }

    expected = {
      base_price: BigDecimal('10'),
      vat: BigDecimal('0.23'),
      discount_percent: BigDecimal('0.2'),
      commission_percent: BigDecimal('0.15'),
      price_with_vat: BigDecimal('12.3'),
      total_price: BigDecimal('9.84'),
      commission: BigDecimal('1.5')
    }

    include_examples 'returns expected values', expected
  end
end
