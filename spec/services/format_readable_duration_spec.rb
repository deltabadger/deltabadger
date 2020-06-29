RSpec.describe FormatReadableDuration do
  describe '#call' do
    subject { described_class.new.call(duration) }
    context 'given 0 seconds' do
      let(:duration) { 0 }

      it { is_expected.to eq('0 seconds') }
    end

    context 'given 60 seconds' do
      let(:duration) { 60 }

      it { is_expected.to eq('1 minute') }
    end

    context 'given 15 minute duration' do
      let(:duration) { 15.minutes }

      it { is_expected.to eq('15 minutes') }
    end

    context 'given 1 hour duration' do
      let(:duration) { 1.hour }

      it { is_expected.to eq('1 hour') }
    end

    context 'given four 15 minute durations' do
      let(:duration) { 15.minutes * 4 }

      it { is_expected.to eq('1 hour') }
    end

    context 'given five 15 minute durations' do
      let(:duration) { 15.minutes * 5 }

      it { is_expected.to eq('1 hour and 15 minutes') }
    end
  end
end
