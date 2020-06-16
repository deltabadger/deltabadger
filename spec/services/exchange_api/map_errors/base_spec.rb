RSpec.describe ExchangeApi::MapErrors::Base do
  describe '#call' do
    class Impl < ExchangeApi::MapErrors::Base
      def errors_mapping
        {
          'rec' => Error.new('recoverable', true),
          'unrec' => Error.new('unrecoverable', false)
        }
      end
    end

    context 'given no errors' do
      subject { Impl.new.call([]) }

      it { is_expected.to eq Impl::Error.new([], true) }
    end

    context 'given unmapped error' do
      subject { Impl.new.call(['unmapped']) }

      it { is_expected.to eq Impl::Error.new(['unmapped'], false) }
    end

    context 'given muliple errors' do
      subject { Impl.new.call(%w[rec unrec unmapped]) }

      it { is_expected.to eq Impl::Error.new(%w[recoverable unrecoverable unmapped], false) }
    end

    context 'given muliple recoverable errors' do
      subject { Impl.new.call(%w[rec rec rec]) }

      it { is_expected.to eq Impl::Error.new(%w[recoverable recoverable recoverable], true) }
    end
  end
end
