import { describe, it, expect, beforeEach, mock } from 'bun:test';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { CustomSelect } from './CustomSelect';

const options = [
  { value: '1', label: 'Option 1' },
  { value: '2', label: 'Option 2', description: 'Desc 2' },
  { value: '3', label: 'Option 3', disabled: true },
];

beforeEach(() => {
  cleanup();
});

describe('CustomSelect', () => {
  it('renders correctly with selected value', () => {
    render(<CustomSelect id="test" options={options} value="1" onChange={mock()} />);
    expect(screen.getByText('Option 1')).toBeTruthy();
  });

  it('renders fallback when value not found', () => {
    render(<CustomSelect id="test" options={options} value="99" onChange={mock()} />);
    expect(screen.getByText('Select...')).toBeTruthy();
  });

  it('opens and shows options when clicked', () => {
    render(<CustomSelect id="test" options={options} value="1" onChange={mock()} />);

    const button = screen.getByRole('button');
    fireEvent.click(button);

    expect(screen.getByRole('listbox')).toBeTruthy();
    expect(screen.getByText('Option 2')).toBeTruthy();
    expect(screen.getByText('Desc 2')).toBeTruthy();
  });

  it('calls onChange and closes when option selected', () => {
    const onChange = mock();
    render(<CustomSelect id="test" options={options} value="1" onChange={onChange} />);

    fireEvent.click(screen.getByRole('button'));
    fireEvent.click(screen.getByText('Option 2'));

    expect(onChange).toHaveBeenCalledWith('2');
    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('closes when blurred outside', () => {
    render(
      <div>
        <CustomSelect id="test" options={options} value="1" onChange={mock()} />
        <button data-testid="outside">Outside</button>
      </div>,
    );

    const trigger = screen.getByRole('button', { name: /Option 1/ });
    fireEvent.click(trigger);
    expect(screen.getByRole('listbox')).toBeTruthy();

    // Trigger blur
    fireEvent.blur(trigger.parentElement!, { relatedTarget: screen.getByTestId('outside') });

    expect(screen.queryByRole('listbox')).toBeNull();
  });
});
