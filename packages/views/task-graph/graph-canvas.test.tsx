import {fireEvent,render,screen} from '@testing-library/react';
import {describe,expect,it,vi} from 'vitest';
import type {DependencyGraphResponse} from '@orvilo/core/types';
import {GraphCanvas} from './graph-canvas';

vi.mock('../issues/components/board-card',()=>({BoardCardContent:({issue}:any)=><div data-shared-board-card={issue.id}><span>{issue.identifier}</span><span>{issue.title}</span></div>}));

const graph={plan:{id:'p'},nodes:[{id:'n1',temp_id:'a',issue_id:'i1',title:'Original plan title',wave:0,status:'in_progress',issue:{id:'i1',identifier:'ACME-4',title:'最新任务标题',status:'in_progress',status_category:'in_progress'}},{id:'n2',temp_id:'b',issue_id:'i2',title:'下一步',wave:1,status:'todo',issue:{id:'i2',identifier:'ACME-2',title:'下一步',status:'todo',status_category:'todo'}}],edges:[{id:'e',from:'a',to:'b',from_issue_id:'i1',to_issue_id:'i2',satisfied:false}]} as unknown as DependencyGraphResponse;
const labels={canvas:'示例依赖图',nodeHint:({identifier,title,state}:{identifier:string;title:string;state:string})=>`${identifier} ${title} ${state}`,edgeHint:()=> 'ACME-4 到 ACME-2',status:(node:{status:string})=>node.status==='in_progress'?'进行中':'待办',empty:'没有任务',undrawn:(count:number)=>`${count} 条未显示的依赖`};
describe('graph-only task nodes',()=>{
 it('reuses board card content and current issue titles without wave labels or secondary text',()=>{
  const {container}=render(<GraphCanvas graph={graph} selection={null} onSelect={()=>{}} labels={labels}/>);
  expect(screen.getByText('最新任务标题')).toBeInTheDocument();
  expect(screen.queryByText('Original plan title')).not.toBeInTheDocument();
  expect(screen.queryByText(/波次|当前没有执行|等待 .* 完成/)).not.toBeInTheDocument();
  const node=screen.getByRole('button',{name:'ACME-4 最新任务标题 进行中'});
  expect(node.textContent).toBe('ACME-4最新任务标题');
  expect(container.querySelectorAll('foreignObject')).toHaveLength(2);
  expect(container.querySelectorAll('[data-shared-board-card]')).toHaveLength(2);
  expect(container.innerHTML).not.toMatch(/fill-blue|fill-amber|stroke-emerald/);
 });
 it('keeps task and dependency keyboard selection',()=>{
  const select=vi.fn();render(<GraphCanvas graph={graph} selection={null} onSelect={select} labels={labels}/>);
  fireEvent.keyDown(screen.getByRole('button',{name:'ACME-4 最新任务标题 进行中'}),{key:'Enter'});
  expect(select).toHaveBeenLastCalledWith({kind:'node',planId:'p',nodeId:'n1'});
  fireEvent.keyDown(screen.getByRole('button',{name:'ACME-4 到 ACME-2'}),{key:' '});
  expect(select).toHaveBeenLastCalledWith({kind:'edge',planId:'p',edgeId:'e'});
 });
});
