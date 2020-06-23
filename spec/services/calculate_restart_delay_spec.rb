RSpec.describe CalculateRestartDelay do
  describe '#call' do
    subject { described_class.new.call(restarts) }

    context 'given 0 restart' do
      let(:restarts) { 0 }

      it 'raises ArgumentError' do
        expect { subject }.to raise_error(ArgumentError)
      end
    end

    context 'given 1 restart' do
      let(:restarts) { 1 }

      it { is_expected.to eq(15.minutes) }
    end

    context 'given 2 restart' do
      let(:restarts) { 2 }

      it { is_expected.to eq(30.minutes) }
    end

    context 'given 4 restart' do
      let(:restarts) { 4 }

      it { is_expected.to eq(2.hours) }
    end

    context 'given 6 restart' do
      let(:restarts) { 6 }

      it { is_expected.to eq(8.hours) }
    end
  end
end
