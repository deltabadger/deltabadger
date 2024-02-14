RSpec.describe PaymentsManager::FlatDiscountCalculator do
  let(:calculator) do
    described_class.new
  end

  subject do
    calculator.call(
      current_plan_base_price: current_plan_base_price,
      current_plan_years: current_plan_years,
      days_left: days_left
    )
  end

  shared_examples 'returns expected value' do |expected|
    it { is_expected.to eq(expected) }

    it { is_expected.to be_a(BigDecimal) }
  end

  context 'given 0 base price' do
    let(:current_plan_base_price) { BigDecimal('0') }
    let(:current_plan_years) { 1 }
    let(:days_left) { 123 }

    include_examples 'returns expected value', 0
  end

  context 'given base price' do
    let(:current_plan_base_price) { BigDecimal('100') }
    let(:current_plan_years) { 1 }
    let(:days_left) { 200 }

    include_examples 'returns expected value', 54.79
  end

  context 'given 0 days left' do
    let(:current_plan_base_price) { BigDecimal('100') }
    let(:current_plan_years) { 1 }
    let(:days_left) { 0 }

    include_examples 'returns expected value', 0
  end

  context 'given more days left than plan days' do
    let(:current_plan_base_price) { BigDecimal('100') }
    let(:current_plan_years) { 1 }
    let(:days_left) { 400 }

    include_examples 'returns expected value', 100
  end

  context 'given 1 year left and 4 year plan' do
    let(:current_plan_base_price) { BigDecimal('100') }
    let(:current_plan_years) { 4 }
    let(:days_left) { 365 }

    include_examples 'returns expected value', 25
  end
end
