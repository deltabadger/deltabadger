require 'rails_helper'

RSpec.describe Payments::CostCalculator do
  describe '#call' do
    shared_examples 'returns expected values' do |expected|
      it 'returns values as BigDecimals' do
        result = subject
        %i[base_price vat discount price_with_vat total_price].each do |param|
          expect(result[param].class).to eq(BigDecimal)
        end
      end

      it 'returns expected base_price' do
        expect(subject[:base_price]).to eq(expected[:base_price])
      end

      it 'returns expected vat' do
        expect(subject[:vat]).to eq(expected[:vat])
      end

      it 'returns expected discount' do
        expect(subject[:discount]).to eq(expected[:discount])
      end

      it 'returns expected price_with_vat' do
        expect(subject[:price_with_vat]).to eq(expected[:price_with_vat])
      end

      it 'returns expected total_price' do
        expect(subject[:total_price]).to eq(expected[:total_price])
      end
    end

    context 'given only base price' do
      let(:args) { { base_price: 20.0 } }
      subject { described_class.new.call(**args) }

      expected = {
        base_price: BigDecimal('20'),
        vat: BigDecimal('0'),
        discount: BigDecimal('0'),
        price_with_vat: BigDecimal('20'),
        total_price: BigDecimal('20')
      }

      include_examples 'returns expected values', expected
    end

    context 'given base price and vat' do
      let(:args) { { base_price: 10.0, vat: 0.2 } }
      subject { described_class.new.call(**args) }

      expected = {
        base_price: BigDecimal('10'),
        vat: BigDecimal('0.2'),
        discount: BigDecimal('0'),
        price_with_vat: BigDecimal('12'),
        total_price: BigDecimal('12')
      }

      include_examples 'returns expected values', expected
    end

    context 'given base price, vat and discount' do
      let(:args) { { base_price: 10.0, vat: 0.2, discount: 0.3 } }
      subject { described_class.new.call(**args) }

      expected = {
        base_price: BigDecimal('10'),
        vat: BigDecimal('0.2'),
        discount: BigDecimal('0.3'),
        price_with_vat: BigDecimal('12'),
        total_price: BigDecimal('8.4')
      }

      include_examples 'returns expected values', expected
    end
  end
end
