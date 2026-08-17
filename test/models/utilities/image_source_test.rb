# frozen_string_literal: true

require 'test_helper'

# The image URL comes from the market-data provider's JSON, so it is upstream text rather
# than anything this app chose. Handing it to ImageMagick as an input specification makes
# ImageMagick fetch it, which turns a field in someone else's API response into a request
# from inside the container network — and a leading dash or a `coder:` prefix changes what
# convert does with it. So the URL is checked and the bytes are fetched here, and only a
# local path ever reaches ImageMagick.
class Utilities::ImageSourceTest < ActiveSupport::TestCase
  ALLOWED = 'https://coin-images.coingecko.com/coins/images/1/large/bitcoin.png'

  test 'refuses a scheme other than https' do
    assert_not Utilities::Image.allowed_source?('http://coin-images.coingecko.com/a.png')
    assert_not Utilities::Image.allowed_source?('file:///etc/passwd')
    assert_not Utilities::Image.allowed_source?('ftp://coin-images.coingecko.com/a.png')
  end

  test 'refuses a host outside the allowlist' do
    assert_not Utilities::Image.allowed_source?('https://evil.example/a.png')
    assert_not Utilities::Image.allowed_source?('https://coin-images.coingecko.com.evil.example/a.png')
  end

  # The reason the SSRF matters here rather than in the abstract: the container shares a
  # Docker network with the market-data service and the other instances.
  test 'refuses hosts reachable only from inside the deployment' do
    assert_not Utilities::Image.allowed_source?('https://data-api:3000/logos/btc.png')
    assert_not Utilities::Image.allowed_source?('https://169.254.169.254/latest/meta-data/')
    assert_not Utilities::Image.allowed_source?('https://127.0.0.1/a.png')
  end

  test 'accepts the market-data provider host' do
    assert Utilities::Image.allowed_source?(ALLOWED)
  end

  test 'accepts the configured market-data host on a hosted install' do
    MarketDataSettings.stubs(:deltabadger_public_url).returns('https://data.example.com')

    assert Utilities::Image.allowed_source?('https://data.example.com/logos/btc.png')
  end

  test 'a malformed url is refused rather than raising' do
    assert_not Utilities::Image.allowed_source?('not a url')
    assert_not Utilities::Image.allowed_source?('')
    assert_not Utilities::Image.allowed_source?(nil)
  end

  # The whole point: whatever the URL, convert is handed a path on this filesystem.
  test 'never hands a remote url to ImageMagick' do
    Utilities::Image.expects(:extract_dominant_colors).never

    assert_nil Utilities::Image.dominant_colors_from_url('https://evil.example/a.png')
  end

  PNG = "\x89PNG\r\n\x1A\n".b + ('x' * 32).b

  test 'downloads an allowed url and passes a local path with the coder made explicit' do
    stub_request(:get, ALLOWED).to_return(body: PNG, status: 200)
    seen = nil
    contents = nil
    # Read inside the stub: the file lives only for the duration of the call.
    Utilities::Image.stubs(:extract_dominant_colors).with do |path, *|
      seen = path
      contents = File.binread(path.sub(/\A[a-z]+:/, ''))
      true
    end.returns(['#ff9900'])

    assert_equal ['#ff9900'], Utilities::Image.dominant_colors_from_url(ALLOWED)
    assert_match(/\Apng:/, seen, 'the coder must be explicit so the argument cannot be reinterpreted')
    assert seen.sub(/\A[a-z]+:/, '').start_with?('/'), 'convert must be handed a local path, never a url'
    assert_equal PNG, contents, 'convert must read the downloaded bytes'
  end

  # The bytes decide the coder, not the URL and not the Content-Type — so a payload dressed
  # as an image never reaches convert as one.
  test 'refuses content that is not a recognised image' do
    stub_request(:get, ALLOWED).to_return(body: '<svg><!ENTITY x SYSTEM "file:///etc/passwd">', status: 200)
    Utilities::Image.expects(:extract_dominant_colors).never

    assert_nil Utilities::Image.dominant_colors_from_url(ALLOWED)
  end

  test 'a download that fails yields no colors rather than raising' do
    stub_request(:get, ALLOWED).to_return(status: 500)

    assert_nil Utilities::Image.dominant_colors_from_url(ALLOWED)
  end

  test 'a missing ImageMagick executable yields no colors rather than aborting asset sync' do
    MiniMagick.stubs(:convert).raises(Errno::ENOENT, 'magick')

    assert_nil Utilities::Image.extract_dominant_colors('/tmp/asset.png')
  end

  # A hostile peer that answers with an unbounded body would otherwise be copied into the
  # container's disk and memory.
  test 'the download is bounded by a size cap' do
    stub_request(:get, ALLOWED).to_return(body: PNG + ('x' * Utilities::Image::MAX_BYTES).b, status: 200)

    assert_nil Utilities::Image.dominant_colors_from_url(ALLOWED)
  end
end
